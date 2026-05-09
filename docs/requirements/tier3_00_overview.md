# Tier 3 Overview — Tooling & Developer Experience

Companion index for the eight Tier-3 requirements documents. Read this first.

## The docs

| # | Feature | Doc |
|---|---------|-----|
| 01 | LSP enhancements (completion, diagnostics-on-edit, inlay hints, signature help, document/workspace symbols, rename, find-references, code actions, LSP formatting) | [tier3_01_lsp.md](tier3_01_lsp.md) |
| 02 | Debugger (DWARF emission + DAP adapter) | [tier3_02_debugger.md](tier3_02_debugger.md) |
| 03 | Test framework (`@[test]`, `riven test`) | [tier3_03_test_framework.md](tier3_03_test_framework.md) |
| 04 | Doc generator (`rivendoc`) | [tier3_04_doc_generator.md](tier3_04_doc_generator.md) |
| 05 | Benchmarking (`@[bench]`, `riven bench`) | [tier3_05_benchmarking.md](tier3_05_benchmarking.md) |
| 06 | Incremental compilation at the core level (query / salsa-style) | [tier3_06_incremental_compile.md](tier3_06_incremental_compile.md) |
| 07 | MIR optimizations (const fold, DCE, simplify, inline) | [tier3_07_mir_optimizations.md](tier3_07_mir_optimizations.md) |
| 08 | Property testing (CLAUDE.md claim audit + recommendation) | [tier3_08_property_testing.md](tier3_08_property_testing.md) |

## Scope boundary vs other tiers

Tier 3 is about *tooling that sits on top of a compiler that works*. It does
not fix language-level gaps (Tier 1), ABI gaps (Tier 2), or CI/distribution
(Tier 4). Wherever a Tier-3 feature is blocked on a Tier-1/2 deliverable, the
dependency is called out in the individual doc's §7 "Interactions" section
and again in the dependency graph below.

Three Tier-3 items also touch the compiler proper:

- **Doc 06 (incremental)** touches every compiler phase because memoization
  has to cut the query graph cleanly.
- **Doc 07 (MIR opts)** adds a new pass pipeline between MIR lowering and
  codegen.
- **Doc 02 (debugger)** adds DWARF emission inside the LLVM (and, to the
  extent possible, Cranelift) codegen paths.

The remaining five (LSP, test, doc, bench, property-testing) are additive
tooling layered on top of the existing API.

## Current state — one-sentence per axis

Full file:line citations live in each doc's §2 "Current state" section. This
is the 30-second summary.

| Axis | Today |
|---|---|
| LSP | `tower-lsp` server with 5 capabilities: `textDocument/didOpen`, `didChange` (FULL sync, no re-analysis), `didSave` (triggers full re-analysis + diagnostics), `hover`, `definition` (same file only), `semanticTokens/full`. No completion, no rename, no references, no inlay hints, no signature help, no document symbols, no code actions, no LSP formatting, no workspace symbols, no `didChangeConfiguration`, no file-watch response. (`crates/riven-lsp/src/server.rs:38-238`) |
| Debugger | Zero. One stub file `crates/riven-core/src/codegen/llvm/debug.rs:1-3` says "Full DWARF debug info will be implemented in a follow-up phase." No DWARF emission in Cranelift. No DAP adapter. No `--emit-debug` flag. No `.debug_info` sections. |
| Test framework | Zero. All tests are Rust `#[test]` inside `crates/riven-core/tests/*.rs` and `crates/*/src/**/tests.rs`. There is no Riven-language `@[test]` attribute, no `riven test` subcommand (see `crates/riven-cli/src/cli.rs:25-114`), no assertion macros, no test runner. |
| Doc generator | Zero. Doc comments `## ...` are lexed into `TokenKind::DocComment` (`crates/riven-core/src/lexer/mod.rs:211-226`) and immediately *discarded* at four sites in the parser (`parser/mod.rs:455-458`, `:884-887`, `:1112-1115`, `:1187-1190`). No `rivendoc` binary. No HTML emission. No search index. |
| Benchmarking | Partial at the Rust level: `rivenc/benches/cache_bench.rs` uses `criterion`. No Riven-language `@[bench]` attribute. No `riven bench` subcommand. No statistical harness. No regression-tracking output. |
| Incremental | **File-level only** via `rivenc/src/cache/`: content-addressed object cache that hashes the full source file + compiler version + flags, and on hit re-uses the compiled `.o` plus a serialized public `FileSignature` (`rivenc/src/cache/mod.rs:1-55`). There is **no query-based memoization** inside `riven-core`. A single-character edit busts the cache for that file and re-runs lex → parse → resolve → typeck → borrow-check → MIR → codegen from scratch. |
| MIR opts | Zero. `crates/riven-core/src/mir/mod.rs:1-4` declares only `nodes` and `lower`. The lowerer produces MIR and hands it straight to codegen — no `const_fold`, no `simplify_cfg`, no `dce`, no `inline` pass module exists. Inkwell's LLVM optimizer (`codegen/llvm/optimize.rs`) runs on the LLVM IR, not MIR — so Cranelift debug builds have no optimization layer at all. |
| Property testing | CLAUDE.md claims "`proptest` for property-based testing in `riven-core`". `proptest` is in `crates/riven-core/Cargo.toml:17` as a dev-dependency. The *only* file that imports it is `crates/riven-core/tests/runtime_safety.rs:61`, and the two `proptest!` blocks there are placeholders: `prop_assert!(expected_len <= 100)` and `prop_assert_eq!(n, n)` (`runtime_safety.rs:68-86`). They assert nothing about actual compiler behavior. The claim is effectively false. |

## Cross-doc dependency graph

```
                    ┌────────────────────────────────┐
                    │ Tier 1 prerequisites           │
                    │ (derive, Drop, stdlib 1a)      │
                    └───────────────┬────────────────┘
                                    │
                    ┌───────────────┴─────────────────┐
                    ▼                                 ▼
       ┌─────────────────────────┐       ┌──────────────────────────┐
       │ 06: Incremental         │       │ 07: MIR opts             │
       │ (query/salsa on top     │       │ (independent of rest;    │
       │  of riven-core)         │       │  improves debug builds)  │
       └────────┬────────────────┘       └──────────────────────────┘
                │
                ▼
       ┌──────────────────────────┐
       │ 01: LSP enhancements     │◀─── 03/04/05: tooling that
       │ (completion, hints,      │      needs rich semantic
       │  rename, references)     │      data also benefits
       └────────┬─────────────────┘
                │
    ┌───────────┼───────────────┬───────────────┐
    ▼           ▼               ▼               ▼
 ┌────────┐ ┌────────┐ ┌────────────────┐ ┌────────────────┐
 │ 03:    │ │ 04:    │ │ 05:            │ │ 08:            │
 │ Tests  │ │ Docs   │ │ Bench          │ │ Proptest       │
 │ (@test)│ │        │ │ (@bench)       │ │ (recommend)    │
 └────────┘ └────────┘ └────────────────┘ └────────────────┘

 02: Debugger — parallel track, depends on Tier-1 stdlib only in the
     sense that good pretty-printers need Debug derive (doc 05).
```

### Key cross-cutting dependencies

- **LSP completion requires some form of incremental analysis.** Today
  `analyze()` re-runs lex → parse → typeck → borrow-check on every
  `didSave` (`crates/riven-lsp/src/server.rs:136`). A meaningful completion
  experience types on every keystroke, which means either (a) ship LSP
  with full-file re-analysis and accept slow response on files >1000 lines,
  or (b) land doc 06 (incremental) first. **We recommend (a) for Phase 1
  and (b) as the follow-up** — completion can be done with `didChange`
  re-analysis gated by a small debounce, because most Riven files in a
  first-party codebase fit under 500 lines and the full pipeline runs in
  well under 50 ms on that size. Inlay hints and type-based completion on
  10k+ line files will need 06.

- **Doc 05 (bench) and doc 03 (test) both need a `@[...]` attribute
  dispatch that actually works.** Today `@[link]` and `@[repr]` are the
  only two attribute forms consumed (`crates/riven-core/src/parser/mod.rs:473-511`).
  `@[derive]` is parsed two ways, consumed zero ways (tier1 §B2). Doc
  03 and 05 *both* need a real attribute pipeline — the same one tier1 doc
  05 specifies. A v1 shortcut that bolts `@[test]`/`@[bench]` into the same
  `if attr.name == "..."` ladder as `@[repr]` buys weeks, but it will have
  to be rewritten when tier1 doc 05 lands. Both docs flag this trade-off.

- **Doc 04 (docs) is gated on deciding what `##` attaches to.** Today
  `## ...` is a `TokenKind::DocComment` that every parser site silently
  throws away. Before `rivendoc` is useful, doc comments need to be
  captured on items (`HirFuncDef`, `HirStructDef`, etc.) and threaded
  through lowering. This is an AST/HIR change, not just a tooling change.

- **Doc 02 (debugger) is only meaningfully shipped on LLVM.** Cranelift
  debug-info support in `cranelift-codegen 0.130` is improving but is not
  production-grade for Riven-level semantics (nested scopes, generic
  instantiations). Ship debugger as `--backend=llvm --debug`-only for v1;
  accept that debug-builds-with-debugger run the slower LLVM backend.
  Debug builds without `--debug` can continue to use Cranelift.

- **Doc 07 (MIR opts) is partially pre-empted by LLVM's own optimizer.**
  Release builds (`--release` → LLVM `default<O2>` at
  `codegen/llvm/optimize.rs:29`) already run DCE/const-fold/inlining on
  LLVM IR. MIR opts mainly matter for (i) Cranelift builds where the
  backend optimizer is weaker, (ii) reducing IR size for faster LLVM
  compilation, and (iii) catching bugs earlier via a simpler IR. This
  shapes the phasing in doc 07: land `const_fold` + `simplify_cfg` + `dce`
  first — they're cheap and help Cranelift; defer MIR-level inlining.

- **Doc 08 (property testing) is a documentation-cleanliness ask.** Either
  fix the CLAUDE.md line ("proptest for property-based testing in
  riven-core") by adding real proptest coverage for lexer / parser /
  borrow-check round-trips, or delete the claim. Doc 08 recommends adding
  real coverage because the lexer and parser are both excellent candidates
  (arbitrary-input-never-panics; roundtrip-formatter-parser).

## Recommended implementation order

**Phase 3A — LSP completion + formatting + diagnostics-on-edit (2-3 weeks).**
Highest-leverage UX wins. No compiler changes required; sits entirely in
`crates/riven-ide` and `crates/riven-lsp`. Expected phases:

1. Wire `riven_core::formatter::format()` into
   `textDocument/formatting`. Formatter already exists as a library API
   (`crates/riven-core/src/formatter/mod.rs:56-128`) — the LSP handler is
   ~30 lines.
2. Move analysis onto `didChange` with a 200 ms debounce, publish
   diagnostics incrementally. Unlocks live error squigglies.
3. Completion — start with scope-aware identifier completion; then field
   completion after `.`; then keyword completion. (See doc 01 phases.)
4. Inlay hints (types on `let` without annotation; parameter names at
   call sites) and signature help.
5. Rename + find-references — both driven by a reverse index from
   `DefId` to use-site `Span`s that the LSP builds during `analyze()`.

**Phase 3B — Doc comments + doc generator (2 weeks).**
Deliver `##`-on-item capture first as a compiler change; ship `rivendoc`
HTML generator second.

1. Thread `doc_comments: Vec<String>` through `ast::TopLevelItem`,
   `ast::ClassDef`, `ast::StructDef`, etc. Stop throwing them away at the
   four parser sites.
2. Surface doc text in LSP hover — cheap win that validates the plumbing.
3. Ship `rivendoc` with minimal HTML + search index. Blocked by Tier-1
   stdlib for self-hosting the stdlib docs, but can ship against any
   user crate first.

**Phase 3C — `@[test]` + `riven test` (1-2 weeks).**
Ship with a compiler-builtin `@[test]` attribute that the typechecker
recognizes without a derive-system prerequisite. Full macro/derive
integration (tier1 doc 05) follows later.

**Phase 3D — MIR opts (2 weeks).**
Add a pass pipeline between MIR lowering and codegen. Ship `const_fold`
+ `simplify_cfg` + `dce` in the first drop. Inline later.

**Phase 3E — `@[bench]` + `riven bench` (1 week).**
Reuses 99% of the test-framework infrastructure. Add statistical harness
(uses `criterion`-style loop + warmup + mean/stddev).

**Phase 3F — Incremental core (salsa-style) (4-6 weeks).**
The largest, riskiest item. Land only after 3A-3E are stable. Introduce
a query layer at the core boundaries: `query_parse(path) ->
Result<ast::Program>`, `query_typeck(ast) -> HirProgram`, etc. Memoize
on content-addressed fingerprints. The file-level cache in
`rivenc/src/cache/` is an existence proof for the fingerprint model but
runs at coarse granularity.

**Phase 3G — Debugger (3-4 weeks).**
DWARF emission via Inkwell's `DebugInfoBuilder`. Ship with a minimal
DAP adapter as a separate crate (`riven-dap`). Works only on LLVM
backend; Cranelift support is a stretch goal.

**Phase 3H — Property testing cleanup (0.5 week).**
Either add real coverage to `runtime_safety.rs` and extend to lexer /
parser / formatter roundtrip, or remove the CLAUDE.md claim. Doc 08
recommends the former and sketches ~3-5 real properties.

## Total estimate

~14-20 weeks for one engineer. Parallelizable across Debugger (G) and
everything else, since G touches only codegen. The LSP track (A) can
also start immediately without blocking on 06/07.

## Open decisions for the project lead

These surfaced in multiple docs and need a single ruling before
implementation begins:

1. **Doc-comment syntax.** Lexer already emits `##` as
   `TokenKind::DocComment` (`crates/riven-core/src/lexer/mod.rs:211-226`).
   Doc 04 proposes keeping `##` (Ruby-flavored) rather than adding `///`
   or `/**`. Decide.
2. **Test attribute spelling.** Tier-1 doc 05 §B2 flagged
   `@[derive(...)]` is parsed wrong today. Doc 03 has the same problem
   for `@[test]`. Pick `@[test]` (matches existing `@[repr]`/`@[link]`)
   or `@test` (no brackets) as canonical now.
3. **Test expectation API.** Assert macros? (`assert_eq!`, `assert!`.)
   Panic-on-fail with a free `expect(actual, expected)` helper? Rust
   uses both; Go uses neither. Pick one.
4. **`riven bench` output format.** `criterion`-compatible JSON, TAP,
   a bespoke format, or all three? Informs doc 05 Phase 2.
5. **Incremental granularity.** Function-level, file-level, or
   module-level memoization? Doc 06 recommends function-level for typeck
   and file-level for lex/parse. Decide before starting 06.
6. **DWARF-in-Cranelift.** Is the shipped Riven debugger `--backend=llvm`-
   only (simpler), or do we commit engineering to Cranelift's
   `cranelift-codegen::debug::write_debuginfo`? Doc 02 recommends LLVM-only
   for v1.
7. **Property-testing scope.** Doc 08 recommends three concrete
   properties: lex-never-panics-on-arbitrary-bytes, parse-never-panics-on-
   arbitrary-tokens, format-parse-format-is-fixpoint. Accept or cut?

## Interactions with earlier tiers

- **Tier 1 doc 05 (derive + macros)** is a soft dependency for docs 03
  (test) and 05 (bench). Both can ship with compiler-builtin
  `@[test]`/`@[bench]` that side-step the full derive system; revisit
  when doc 05 lands.
- **Tier 1 doc 01 (stdlib)** is a hard dependency for doc 03: assertion
  macros need `fmt::Debug` to print values on failure, and `panic!` /
  `process::exit` must be available to abort a failing test. Ship
  assertions v1 against a minimal internal `__test_panic(msg)` shim
  until stdlib catches up.
- **Tier 1 doc 04 (Drop)** has to land before `riven test` can run tests
  that allocate — otherwise tests that pass today will leak enough memory
  to OOM a CI runner given enough iterations.
