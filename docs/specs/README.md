# Riven Specs

Riven uses **Spec-Driven Development (SDD)**.  Every feature flows:

```
docs/requirements (or tutorial / prompts)
        ↓
docs/specs/<area>/<feature>.spec.md         ← this directory
        ↓
crates/riven-core/tests/*.rs (pin tests)
        ↓
implementation
```

Specs are the **source of truth**.  Tests are the **enforcement
layer** — every numbered behaviour (B1, B2, …) in a spec has at least
one Rust integration test or release-e2e fixture that pins it.

## Spec file shape

Each `.spec.md` follows the same skeleton:

```markdown
# Spec — <area>::<feature>

**Source docs:** links to docs/requirements + docs/prompts
**Status:** when it shipped + phase tag

(short intro)

## B1 — <one-line behaviour title>

Given / When / Then in prose.

## B2 — …

…

## Pin tests

| Behaviour | Test fn | File |
|-----------|---------|------|
| B1        | …       | …    |

## Out of scope (v2)
- deferred items, with reasons
```

The "Pin tests" table is the contract — every `B<n>` must appear in
at least one row.  If a behaviour ships without a pin test, list it
in a **Gaps** section so the missing coverage is visible.

## Adding a new feature

1. **Identify the source doc.**  Usually `docs/requirements/tier*` or
   `docs/prompts/v1/`.  If none exists, write the doc first.
2. **Write the spec.**  Create `docs/specs/<area>/<feature>.spec.md`
   with numbered behaviours.  Submit for review before opening any
   source file.
3. **Implement TDD.**  For each `B<n>`, write the pin test first,
   watch it fail, then implement until it passes.  Add the test to
   the spec's Pin tests table.
4. **Iterate.**  `cargo test --workspace` must be green at every
   commit.  If a behaviour changes, update the spec first.

## Backfilling

Specs are being backfilled for already-shipped features as we touch
them.  Pin tests usually already exist as Rust integration tests —
cross-link rather than duplicate.

## Index

### Phase 2 #06 — stdlib

- [std::fmt](stdlib/fmt.spec.md) — Display, Debug, Formatter,
  interpolation routing (D2), format specs (D4).
- [std::io](stdlib/io.spec.md) — Stdin, Stdout, Stderr, IoError.
- [std::env](stdlib/env.spec.md) — args, var, vars, current_dir.
- [std::fs](stdlib/fs.spec.md) — read_to_string, write, read_dir,
  predicates.
- [std::process](stdlib/process.spec.md) — exit, process_run.

### Future (backfill as we touch them)

- std::path, std::time, std::net, std::hash — not yet spec'd.
- Trait system, ownership / borrow checker — not yet spec'd.
- Codegen backends (Cranelift / LLVM) — not yet spec'd.

## Cross-references

- Long-form requirements: [docs/requirements/](../requirements/).
- Phase prompts (driver scripts): [docs/prompts/v1/](../prompts/v1/).
- Implementation plans: [docs/superpowers/plans/](../superpowers/plans/).
- Tutorial (user-facing): [docs/tutorial/](../tutorial/).
