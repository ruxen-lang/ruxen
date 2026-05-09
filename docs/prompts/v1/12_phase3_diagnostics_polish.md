# 12 — Phase 3: diagnostic polish (T5.03 deprecation + T5.05 suggestions)

**Depends on:** prompt 11.
**Reads:** `docs/requirements/tier5_03_deprecation_stability_attrs.md`,
`docs/requirements/tier5_05_diagnostic_suggestions.md`.

## A. T5.03 — `@[deprecated]` and `@[stable]` / `@[unstable]` attrs

### Goal

```riven
@[deprecated(since = "0.5.0", note = "use foo_v2 instead")]
def foo_v1() -> Int = 0
```

### TDD
- Parser test: `@[deprecated(...)]` parses with named args.
- Typeck test: calling a deprecated fn emits warning E1300.
- E2E fixture asserts warning text.

### Implementation
- Extend `Attribute` arg structure to support `name = value` pairs
  (already split per P0.4).
- Resolve pass tags `DefKind::Function { ... deprecated: Option<...> }`.
- Typeck visits every call and emits a warning on deprecated targets.

## B. T5.05 — suggestion framework

### Goal

When a diagnostic has a fix, attach a `suggestion` block:

```
error[E0XXX]: undefined variable `printlnn`
   |
12 |   printlnn "hi"
   |   ^^^^^^^^
help: did you mean `println`?
   |
12 |   println "hi"
   |   ~~~~~~~
```

### TDD
- Unit test for `did_you_mean(input, candidates)` with edit distance
  threshold ≤ 2.
- Test rendering: diagnostic with one suggestion produces help block.
- E2E fixture: run compiler on a typo, capture stderr, assert
  suggestion present.

### Implementation
- Extend `Diagnostic` with `suggestions: Vec<Suggestion>`. Each
  suggestion has `span` and `replacement: String`.
- Add `did_you_mean` helper used by resolve / typeck for unknown
  names.
- LSP `publishDiagnostics` carries `relatedInformation` /
  `codeActions` from suggestions (prompt 10 capability extension).

## Reserved error / warning codes

- E1300 — use of deprecated item (warning)
- E1301 — use of unstable item (warning, gated on flag)
- E1302 — `@[deprecated]` on non-callable item (error)

## Definition of done

- [ ] `@[deprecated]` and stability attrs parse, validate, warn.
- [ ] Suggestions render in CLI output and in LSP `codeActions`.
- [ ] At least 5 diagnostics across the compiler now carry
      suggestions (typo on identifier, missing `mut`, missing `&`,
      wrong return type, missing `?`).
- [ ] CI green.
- [ ] CHANGELOG bullet.
