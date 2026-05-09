# Tier 5 Roadmap — Specification & Long-Term Language Health

Companion index for the five Tier-5 requirements documents. Read this first.

## The docs

| # | Feature | Doc |
|---|---------|-----|
| 01 | Language reference (formal grammar + normative prose) | [tier5_01_language_reference.md](tier5_01_language_reference.md) |
| 02 | Edition / stability mechanism (`edition = "2026"`) | [tier5_02_edition_stability.md](tier5_02_edition_stability.md) |
| 03 | Deprecation / stability attributes on APIs | [tier5_03_deprecation_stability_attrs.md](tier5_03_deprecation_stability_attrs.md) |
| 04 | Error-code registry + `riven --explain` | [tier5_04_error_code_registry.md](tier5_04_error_code_registry.md) |
| 05 | Diagnostic suggestions framework (`help:` + replacement) | [tier5_05_diagnostic_suggestions.md](tier5_05_diagnostic_suggestions.md) |

---

## 1. What Tier 5 is and why it matters

Tiers 1-4 fill functional gaps: stdlib, concurrency, async, LSP, macros, and so
on. Tier 5 is about **what a language needs to survive past v1**:

- A **normative reference** so an independent implementer could build a
  conforming compiler without reverse-engineering `riven-core`.
- A **stability and versioning story** so libraries written today keep
  compiling tomorrow, and breaking improvements can still ship.
- **Deprecation and feature-gating attributes** so stdlib can grow without
  freezing every wart into permanence.
- **Legible error output with stable codes and long-form explanations** — the
  single most cited reason Rust won user affection, and uniquely important
  for Riven because borrow-check + lifetime errors are the hardest diagnostics
  any language emits.
- A **structured suggestion framework** (span + replacement + confidence) so
  the LSP (tier 3), `riven fix` migrator (tier 5 §02), and terminal renderer
  can all share one source of truth.

None of this is urgent for a prototype. All of it is the difference between
"cool side project" and "language someone bets a company on."

---

## 2. Consumers of the spec

| Consumer | What they need |
|----------|----------------|
| **External implementers** (alt-compilers, interpreters, transpilers) | A complete, normative grammar + type-system description; error codes they can emit compatibly. |
| **Tooling** (LSP, formatter, docs generator, `riven fix`) | A machine-readable or stable surface: error codes with categories, suggestion payloads, stable AST shape per edition. |
| **Users** (debugging errors) | `riven --explain E0308` long-form, hover-docs in LSP, deprecation warnings with upgrade paths. |
| **Compiler contributors** | Know which rules are normative vs informative; know where to register new error codes; know what an edition gate looks like. |
| **Library authors** | `@[stable(since = "0.3")]`/`@[deprecated(since = "0.5", note = "use `Foo::new`")]` so they can evolve APIs without breaking downstream. |

---

## 3. Current state (summary)

The fine-grained citations are in the per-doc §2 sections. Top-level status:

- **Language reference**: none. There is no `docs/reference/`, no `SPEC.md`,
  no `GRAMMAR.md`. The user-facing docs are `docs/tutorial/01-16`, which are
  informative-only and incomplete even as a tutorial (e.g. precedence is
  never shown; only hinted at in `docs/tutorial/02-variables-and-types.md`).
- **Editions**: the field `package.edition: Option<String>` exists
  (`crates/riven-cli/src/manifest.rs:29`) and `"2026"` is written by the
  scaffolder (`scaffold.rs:145`). It is **never read** by the compiler —
  grep `edition` across `crates/riven-core` returns zero matches outside of
  Rust's own `Cargo.toml` files. No edition-gating logic exists.
- **Deprecation / stability attributes**: no attribute named `deprecated`,
  `stable`, or `unstable` is recognised. `parse_attributes` only
  dispatches `@[link]` and `@[repr]` (`parser/mod.rs:473-511`). No warning
  infrastructure for "use of deprecated X."
- **Error codes**: partial. `BorrowError` has `ErrorCode::E1001..E1010`
  (`borrow_check/errors.rs:6-16`) and renders `error[E1001]:` headers
  (`:66`). `Diagnostic::error_with_code` exists (`diagnostics/mod.rs:39`) but
  **no other subsystem uses it** — the typeck emits plain `Diagnostic::error`
  with no code. No `--explain` subcommand anywhere in `rivenc` or
  `riven-cli/src/cli.rs:25-113`. No error-explanation registry.
- **Diagnostic suggestions**: ad-hoc. `BorrowError.help: Vec<String>`
  (`borrow_check/errors.rs:61`) is free-form prose. No span-based
  replacement, no machine-applicability level, no structured suggestion
  type. The LSP (`riven-ide/src/diagnostics.rs:30-60`) only maps primary
  span + secondary "related information"; `help` entries are lost in the
  LSP conversion (`:47-58` does not touch `err.help`).

---

## 4. Cross-doc dependency graph

```
          ┌───────────────────────────────────────────┐
          │  03: Deprecation / stability attributes    │
          │   (attribute surface, warning emission)    │
          └─────────────────┬─────────────────────────┘
                            │
                ┌───────────┼─────────────┐
                ▼           ▼             ▼
    ┌───────────────────┐ ┌───────────┐ ┌───────────────────┐
    │ 05: Suggestions    │ │ 04: Error │ │ 02: Editions      │
    │  (need code slot + │ │ codes +   │ │ (migration lints  │
    │  applicability)    │ │ --explain │ │  are deprecations)│
    └─────────┬──────────┘ └─────┬─────┘ └─────────┬─────────┘
              │                  │                 │
              └──────────┬───────┴─────────────────┘
                         ▼
          ┌───────────────────────────────────────────┐
          │  01: Language reference                    │
          │   (consolidates the above: normative       │
          │    grammar, error codes, attributes,       │
          │    edition deltas)                         │
          └───────────────────────────────────────────┘
```

**Read order when implementing:**

1. **03** (attributes) first — blocks the other four, because editions,
   deprecations, `#[allow(…)]` on suggestions, and `#[stable]` in the
   language reference all use attribute machinery.
2. **04** (error codes) and **05** (suggestions) together — they share a
   `Diagnostic` shape. Doing them independently will create two incompatible
   `Diagnostic` types.
3. **02** (editions) once attributes exist — edition-gated features use the
   `#[unstable(feature = "…")]` attribute from doc 03.
4. **01** (language reference) last — consolidates all of the above and
   requires a stable surface to describe.

---

## 5. Cross-tier interactions

| Tier 5 doc | Interacts with |
|------------|----------------|
| 01 Language reference | **All tiers.** Parser (tier 2 if any), typeck, borrow check, stdlib (tier 1). The reference must be updated whenever surface changes. |
| 02 Editions | **Tier 1** (stdlib — old-edition shims); **Tier 3** (LSP — edition-aware diagnostics); **Tier 4** (`riven-cli` — manifest). |
| 03 Attributes | **Tier 1** (derive macros — share `@[...]` surface); **Tier 1.05** (stability attrs on every stdlib item); **Tier 3** (LSP — hover shows stability). |
| 04 Error codes | **Tier 3** (LSP — `diag.code` already flows through `riven-ide/src/diagnostics.rs:22`); all phases that emit errors. |
| 05 Suggestions | **Tier 3** (LSP code actions = quick fix); **tier 5 doc 02** (`riven fix` migrator reuses machine-applicable suggestions). |

---

## 6. Recommended implementation order

**Phase A — attribute framework (1-2 weeks).**
Doc 03 phase 3a. Widen `ast::Attribute.args` from `Vec<String>` to
`Vec<AttrArg>`; add `@[deprecated]`, `@[stable]`, `@[unstable]` recognition;
add a warning-emission pass that fires on use of `deprecated` items.
Unblocks tier-1 B2 (derive attribute untangling) and all other Tier-5 docs.

**Phase B — diagnostic infrastructure unification (1-2 weeks).**
Doc 04 phases 4a-4b and doc 05 phases 5a-5b, interleaved:

1. Introduce `riven-core::diagnostics::v2::Diagnostic` with codes,
   primary/secondary labels, suggestions.
2. Migrate `BorrowError` into it; delete `BorrowError` as a separate type
   (or keep as `pub use` alias for now).
3. Introduce `ErrorCode` as a crate-wide enum covering typeck, borrow check,
   resolve, parser, lexer.
4. `riven --explain` subcommand in `rivenc/src/main.rs` (and mirror in
   `riven-cli`).

**Phase C — error-code population (2-3 weeks).**
Doc 04 phase 4c. Every `self.error(…)` site in resolve/typeck/parser/lexer
gets an `ErrorCode`. This is mechanical but large — roughly 400 call sites
across the compiler. Long-form `.md` explanations land alongside each code
in `docs/errors/E????.md`, enforced by a test that every code has a file.

**Phase D — suggestion callsites (1-2 weeks).**
Doc 05 phase 5c. Upgrade the most-valuable diagnostics to produce
structured suggestions:

- `E0308` (type mismatch) → "consider borrowing: `&expr`".
- `E1001` (use after move) → "consider cloning: `expr.clone`".
- `E0425` (unresolved name) → "did you mean `…`?" via Levenshtein.
- `E0603` (private item) → "make `X` public" (span = the `def` keyword).

**Phase E — edition infrastructure (2-3 weeks).**
Doc 02 phases 2a-2b. Wire `package.edition` through the pipeline; introduce
`EditionCtx`; add the first edition-gated feature (something cheap — e.g.
rename `Hash[K,V]` to `HashMap[K,V]` on 2027 only, issue deprecation on
2026). Implement `riven fix --edition=2027` driver that applies machine-
applicable suggestions across the whole crate.

**Phase F — language reference drafting (4-6 weeks).**
Doc 01 phases 1a-1d. Build `docs/reference/` scaffolding; import precedence
table from `parser/expr.rs:10-51`; write normative prose chapter by chapter.
Add a test harness that every grammar rule has a fixture under
`tests/reference/`.

**Phase G — public polish (ongoing).**
- Spec CI: markdown-lint, broken-link check, fixture-coverage check.
- Website that renders `docs/reference/` + `docs/errors/`.
- `Riven.toml` MSRV (`riven = ">=0.2.0"`) already exists
  (`manifest.rs:32`) — wire it into the compiler so old compilers refuse
  too-new editions cleanly.

**Total estimate:** ~12-18 weeks for a single engineer. Phase A gates the
rest; phases B/C/D are the biggest user-visible deliverable and can ship
in `v0.2`; phase E is `v0.3`; phase F is ongoing documentation work
running in parallel from phase B onward.

---

## 7. Cross-cutting decisions (needed before any doc's implementation)

These surfaced in two or more docs and need a single ruling. Each links to
the relevant doc §OQ for the full argument.

1. **Attribute arg grammar.** Today `ast::Attribute.args: Vec<String>`
   stores everything as strings (`parser/ast.rs:789-793`). Must widen to
   `Vec<AttrArg>` where `AttrArg` is `Literal | KeyValue | Nested`. All
   Tier-5 attributes (`@[deprecated(since = "0.3", note = "use X")]`,
   `@[stable(feature = "foo", since = "0.5")]`, `@[unstable(feature = "y",
   issue = "123")]`) need key-value args. See **doc 03 §OQ-1**.

2. **Error-code number space.** `E1001-E1010` already ship
   (`borrow_check/errors.rs:6-16`), and tier-1 docs reserve `E1011-E1016`
   (concurrency, `tier1_02_concurrency.md:785-790`) and `E0601-E0609`
   (derive, `tier1_05_derive_macros.md:718`). **Doc 04 §4.2** proposes:

   - `E0001-E0999` — parser/lexer/general.
   - `E0100-E0499` — resolve/name resolution.
   - `E0500-E0999` — type check / inference / coercion.
   - `E1000-E1999` — borrow check / lifetimes / ownership (already in use).
   - `E2000-E2999` — MIR / unreachable-code / const-eval.
   - `E3000-E3999` — codegen.
   - `E4000-E4999` — linker / cross-target.
   - `E9000-E9999` — experimental / not-yet-assigned (e.g. doc 05 stub
     codes).

   Rust-compatible numbering (like `E0308`) is *tempting* but **rejected**
   — Rust's numbers are not contractually stable and would create an
   expectation of semantic parity we can't keep. Riven owns its registry.

3. **Suggestion-confidence enum.** Rust has five levels
   (`MachineApplicable`/`MaybeIncorrect`/`HasPlaceholders`/`Unspecified`).
   **Doc 05 §4.3** proposes **three** for Riven:
   `MachineApplicable` / `MaybeIncorrect` / `HasPlaceholders`. Editors
   only auto-apply the first.

4. **`--explain` storage.** **Doc 04 §4.4** proposes:
   - Source of truth: `docs/errors/E????.md`, Markdown-formatted.
   - Compiled into the `rivenc` binary as a `phf` or `include_str!` map at
     build time (one `include_str!` per file, gated by an `explain-embed`
     feature so a slim compiler build can skip it).
   - `riven --explain E0308` looks up the string and pipes it through a
     pager if stdout is a TTY.
   - The CI lint already mentioned in phase C enforces that every
     registered `ErrorCode` has a matching `.md` file.

5. **Editions: good for Riven?** Go's thesis (no editions) prioritises
   total backward compat at the cost of bad names living forever. Rust's
   thesis (editions) prioritises surface-level cleanup at the cost of
   multi-edition compatibility burden on the compiler. **Doc 02 §4.1**
   picks **Rust's model with stricter rules**:
   - An edition may only change *syntax* (new keyword, deprecated
     syntax becomes an error) — it may **not** change semantics of
     existing constructs.
   - Two editions supported at a time: current + previous. Third edition's
     release sunsets the oldest.
   - Cross-edition linking must work (an old-edition library is callable
     from a new-edition crate), because Rust's experience shows this is
     non-negotiable.

6. **Grammar formalism.** **Doc 01 §4.2** proposes **EBNF-with-PEG-style
   extensions** (`?`, `*`, `+`, alt `|`, grouping), plus prose carve-outs
   for the contextual rules Pratt-style parsing embeds (precedence in
   `parser/expr.rs:10-51`, newline-as-terminator in
   `lexer/token.rs:227`). Pure PEG is rejected because it hides ambiguity
   by fiat; pure BNF is rejected because it can't express repetition
   compactly; ANTLR is rejected because the reference is a document, not a
   runnable parser generator.

7. **Spec-to-compiler sync.** **Doc 01 §5.3** recommends the
   **test-fixture approach**: every production rule in the reference has
   at least one `tests/reference/<section>_<n>.rvn` fixture that the
   parser must accept (or reject, with a specific error code). A lint in
   CI enforces 100% rule coverage. This is a middle ground between
   "generated from the parser grammar" (brittle, rules don't map 1-to-1)
   and "pure prose discipline" (rots immediately).

---

## 8. Open questions still not resolved

Items below survived the docs and need the project lead's call:

- **Attribute naming style: `@[deprecated]` vs `@[deprecated(...)]` vs
  bare-arg `@[deprecated "since 0.3"]`**. Docs recommend keyword-argument
  form (`since = "0.3"`, `note = "..."`) matching Rust. See doc 03 §OQ-2.
- **Stability track for the stdlib itself.** Tier-1 stdlib ships every
  function as `@[stable(since = "0.2")]`? Or ships some with
  `@[unstable(feature = "foo")]` requiring a feature gate? Rust uses
  nightly for unstable; Riven has no nightly channel today. Doc 03 §OQ-4.
- **Whether `--explain` should be on `rivenc` or `riven` or both.** Rust
  exposes `rustc --explain` only. We currently have `rivenc` (compiler)
  and `riven` (build tool / `cargo` equivalent). Doc 04 §OQ-2 recommends
  both, with `riven explain` being the canonical user-facing name.
- **Public-ID vs private-ID for spans in serialized diagnostics.** JSON-out
  for `riven-ide` needs a stable shape if people build tooling on it;
  today `Span` includes byte offsets, which are source-version-sensitive.
  Doc 05 §OQ-3.

---

## 9. Acceptance criteria (tier-level)

Tier 5 is "done" when:

- [ ] `docs/reference/` renders at a stable URL, covers lex/parse/types/
      ownership/trait-resolution/coercions/editions, and has a test-fixture
      backing every rule.
- [ ] `riven --explain E1001` prints a paragraph of Markdown-rendered prose
      and an example; same for every other registered code.
- [ ] Every `Diagnostic::error(...)` call in `riven-core` carries an
      `ErrorCode`.
- [ ] `@[deprecated]` on a stdlib item causes call sites to produce a
      warning diagnostic with the `note` and a suggestion to migrate.
- [ ] `@[stable(since = "0.2.0")]` and `@[unstable(feature = "foo")]` are
      parsed, stored on items, and queryable from `riven-ide` hovers.
- [ ] Two editions (`"2026"`, `"2027"`) are live; a crate on `"2026"` can
      be linked against from a crate on `"2027"`; a 2027-only feature is
      gated out when the manifest says `edition = "2026"`.
- [ ] `riven fix --edition=2027` auto-rewrites a small sample project end-
      to-end using machine-applicable suggestions, and leaves
      maybe-incorrect ones flagged.
- [ ] LSP hovers show stability and deprecation info; LSP code actions
      surface machine-applicable suggestions.
