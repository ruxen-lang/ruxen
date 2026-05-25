# 24 — Phase 5: edition mechanism (T5.02)

**Depends on:** prompts 01-15 (need real changes to gate on
editions).
**Reads:** `docs/requirements/tier5_02_edition_stability.md`.

## Goal

Open Decision #6 ruling: ship editions. `Ruxen.toml`'s `edition`
field already validates (P0.11). Wire actual semantic gating.

## Surface

- `edition = "2026"` is the v1 default.
- `edition = "2028"` will be the v2 default. Don't ship 2028 yet;
  ensure the framework accepts an unknown future edition with a
  warning (forward-compat).
- `EditionLint` framework: each lint flags a syntax pattern that
  changes meaning across editions; `ruxen fix --edition 2028` would
  rewrite source to the next edition (v2 feature; scaffold only).

## TDD

- Unit test: `parse_edition` rejects unknown editions
  (`E0042`-equivalent — pick a code).
- Unit test: a hypothetical `keyword_in_2028` lint fires when
  source uses an identifier reserved in 2028.
- Integration test: `ruxen fix --edition 2028 --dry-run` produces
  diff output.

## Implementation

- New `crates/ruxen-core/src/lints/` module.
- `EditionLint` Rust trait (internal compiler API):
  `fn check(item: &HirItem, ctx: &mut LintCtx)`.
- Rules registered per edition.
- The legacy Hash → Map rename (P0.6) is the first edition-lint
  canary — pre-2026 saw the legacy `Hash[K,V]` spelling, 2026 sees
  `Map[K,V]`. Test that loading 2026 source rejects the legacy
  `Hash[K,V]` spelling.

## Reserved error codes

- E1700 — edition lint failure
- E1701 — feature requires newer edition
- E1702 — `ruxen fix` rewrite failed

## Definition of done

- [ ] Edition field gates at least one observable behavior (the
      legacy Hash / current Map canary).
- [ ] `ruxen fix --edition <Y>` scaffold compiles (full
      implementation in v2).
- [ ] CHANGELOG bullet.
