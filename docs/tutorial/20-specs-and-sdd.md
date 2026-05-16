# Specifications and Spec-Driven Development

Riven uses **Spec-Driven Development** (SDD).  Every feature flows:

```
docs/requirements (or tutorial / prompts)
        ↓
docs/specs/<area>/<feature>.spec.md
        ↓
crates/riven-core/tests/*.rs (pin tests)
        ↓
implementation
```

Specs are the **source of truth**.  Tests are the **enforcement
layer** — every numbered behaviour (B1, B2, …) in a spec has at least
one Rust integration test or release-e2e fixture that pins it.

---

## 1. Where the specs live

[`docs/specs/`](../specs/) is the spec layer.  Six subdirectories
mirror the major surface areas:

| Subdir         | Covers                                                  |
|----------------|---------------------------------------------------------|
| `stdlib/`      | Every shipped stdlib module + collection                |
| `mixins/`      | Mixin system, implicit-include, variance                |
| `ownership/`   | Borrow check + Drop                                     |
| `codegen/`     | Cranelift + LLVM backends, runtime safety, FFI surface  |
| `types/`       | In-flight type-system features (const generics, …)      |

Plus [`docs/specs/README.md`](../specs/README.md) — the spec index +
SDD workflow note.

---

## 2. Reading a spec

Every spec follows the same skeleton.  Open
[`docs/specs/stdlib/fmt.spec.md`](../specs/stdlib/fmt.spec.md) as
the canonical example:

1. **Source docs:** links back to the long-form requirements doc
   and the phase prompt that drove the work.
2. **Status:** when it shipped + the phase tag.  (`shipped Phase 2 #06.D4`)
3. **Numbered behaviours.**  Each `## B<n> —` section is one
   testable claim in prose + Given/When/Then form.  These are the
   compiler's promises to the user.
4. **Pin tests table.**  Maps every `B<n>` → the test fn that
   proves it.  If this table is missing a row, the behaviour isn't
   actually enforced — file a gap.
5. **Gaps.**  Behaviours that ship without a dedicated pin test.
   Explicit so the next person knows what to add.
6. **Out of scope (v2).**  Surface that *won't* land in v1, with the
   reason.

When the spec and the implementation disagree, **the spec wins** —
either the implementation has a bug or the spec is out of date.
Either way, fix it; don't let the two drift.

---

## 3. Adding a new feature (the SDD cycle)

1. **Docs first.**  Find or write the source-of-truth doc in
   `docs/requirements/`, `docs/tutorial/`, or `docs/prompts/v1/`.
2. **Spec second.**  Create `docs/specs/<area>/<feature>.spec.md`
   with numbered behaviours.  Get alignment on the spec before
   touching any source file.
3. **Red test.**  For each behaviour, write the pin test first.
   Run `cargo test`; it must fail with a clear error explaining what
   the spec promises but the code doesn't deliver yet.
4. **Implementation.**  TDD as usual — minimum code to make the
   test pass, no extra abstractions.
5. **Green + commit.**  Add the test to the spec's Pin tests table
   in the same commit as the implementation.
6. **Repeat per behaviour.**  Each `B<n>` is one cycle.  Multi-stage
   features (like const generics, with 9 stages) commit per stage —
   each stage adds its own spec section + tests + code.

This isn't TDD with extra paperwork.  The spec is leverage: it lets
you reach alignment about *what* the code is meant to do before
debating *how* it should look.  Most bugs come from drift between
intent and implementation; the spec is the contract that pins both
sides.

---

## 4. The pin-test pattern

A pin test names the spec behaviour it pins.  Two conventions exist
side by side:

**Convention A — function name.**

```rust
/// B3: Stdin.lines() handles partial final line + empty input.
#[test]
fn stdin_lines_no_trailing_empty_and_partial_final_line() { ... }
```

**Convention B — doc comment.**

```rust
/// Pins: docs/specs/stdlib/fmt.spec.md §B7 (width / align / fill).
#[test]
fn interpolation_width_right_align_pads_int() { ... }
```

Both work.  Convention A is denser; B is more discoverable from `grep
"§B"`.  Pick whichever fits the surrounding file.

---

## 5. Backfilling specs for shipped code

When you walk into a feature that's already implemented but has no
spec, **write the spec first**.  Use existing tests as the source of
truth — they're the only thing that today encodes the contract.

The procedure:

1. List every `#[test]` fn in the relevant `crates/riven-core/tests/<name>.rs`.
2. Group them by behaviour.  Multiple tests often pin the same
   B<n>; that's fine.
3. Write the spec with each behaviour numbered.
4. Fill the Pin tests table with the existing test names.
5. If a test doesn't fit any behaviour, either invent the behaviour
   (the test pins it implicitly) or note the test as suspect
   (over-asserts? wrong layer?).

Done in this repo for the entire Phase 2 #06 surface and beyond.
See [commits 8081e63 and af36f23](../specs/README.md) for the
backfill commits.

---

## 6. Gap lists

Every spec ends with a `## Gaps` section listing behaviours that
ship without a dedicated pin test.  This is *not* a TODO list — it's
a contract failure marker.  A behaviour in `Gaps` is one that the
spec promises but no test currently enforces; a future regression
could land silently.

When you have spare cycles, walk the Gaps sections and convert each
into a pin test.  See the recent env / fs / process / vec gap-fill
commits for the pattern.

---

## 7. Out-of-scope sections

Specs explicitly list what they **don't** cover.  These come from
the requirements docs' non-goals plus pragmatic v1 cutoffs:

- `BTreeMap` (no ordered hash variant in v1)
- `fs.metadata` (needs a `Metadata` struct surface)
- Width on `:?` Debug interpolation (Debug bypasses the Formatter)
- ...and so on, per spec.

These are the v2 backlog.  If you encounter a use case the spec
flags as out-of-scope, the spec gets updated *first* — never write
code for an out-of-scope feature without changing the spec.

---

## 8. Reading the implementation through the spec

A working flow for reading unfamiliar code in this repo:

1. Find the relevant `docs/specs/<area>/<feature>.spec.md`.
2. Skim the numbered behaviours to understand the promises.
3. For each behaviour you care about, click into the pin test it
   names.
4. The pin test's body shows you the input → expected output; that
   tells you which source files matter.
5. Open the source file and read the implementation with the
   contract already in your head.

This is roughly 10× faster than reading the source cold.  The spec
gives you intent; the test gives you a concrete example.  The code
is just the materialisation.

---

## 9. Where SDD differs from TDD

| Axis              | TDD                                | SDD                                |
|-------------------|------------------------------------|------------------------------------|
| Source of truth   | Tests                              | Specs (markdown), pinned by tests  |
| Discoverability   | Find the test, infer the contract  | Read the spec, follow to the test  |
| Multi-stage       | Tests evolve as code grows         | Spec lists all stages up front     |
| Coverage gaps     | Implicit (untested code)           | Explicit (`## Gaps` sections)      |
| Out-of-scope      | Implicit (no test exists)          | Explicit (`## Out of scope (v2)`)  |

SDD is TDD plus a document layer.  The document layer is small —
each spec is a few hundred words — but it carries enormous leverage
when you're a year out from a feature and trying to remember why it
behaves the way it does.

---

**Next:** [Chapter 21 — Const Generics (in flight)](21-const-generics.md)
to see SDD in action on a feature that's mid-implementation.
