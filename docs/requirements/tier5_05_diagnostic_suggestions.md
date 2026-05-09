# Tier 5 — Diagnostic Suggestions Framework

Status: draft
Depends on: tier5_04 phase 4a (unified Diagnostic). Independent of
tier5_03 but synergistic: deprecation warnings will carry suggestions.
Blocks: LSP code actions (tier 3), `riven fix` migrator (tier5_02),
human-readable multi-line diagnostic output.

---

## 1. Summary & motivation

The difference between "the compiler tells you what went wrong" and
"the compiler tells you what to do about it" is the difference between
rustc and most other tools. The way Rust does this is a structured
**suggestion** attached to each diagnostic: a span, a replacement
string, a confidence level.

The LSP consumes the suggestion as a **code action** — the user hits
`Cmd-.` and the edit is applied. CI tools consume it as
**machine-applicable fixes**. The compiler itself renders it as a
colourised `help:` line. All from one source of truth.

Riven today has a primitive free-form `help: Vec<String>` on
`BorrowError` (`crates/riven-core/src/borrow_check/errors.rs:61`):

```rust
help: vec![format!("consider cloning the value: `{}.clone`", name)]
```

Good as human prose, useless as a machine-applicable fix. The LSP
conversion (`crates/riven-ide/src/diagnostics.rs:30-60`) doesn't even
propagate `help` into the LSP diagnostic — it's just dropped. No
`textDocument/codeAction` integration is possible.

This doc specifies a structured `Suggestion` type, its applicability
levels, the API for emitting it, terminal rendering, LSP mapping, and
usage guidelines.

---

## 2. Current state

### 2.1 Existing "help" plumbing

`crates/riven-core/src/borrow_check/errors.rs:50-62`:

```rust
pub struct SpanLabel {
    pub span: Span,
    pub label: String,
}

pub struct BorrowError {
    pub code: ErrorCode,
    pub primary: SpanLabel,
    pub secondary: Vec<SpanLabel>,
    pub help: Vec<String>,       // ← free-form text
}
```

Use sites in `borrow_check/mod.rs`:

- `:386` — "consider cloning the value: `{name}.clone`"
- `:500`, `:525` — "consider declaring with `let mut {name}`"
- `:684`, `:715` — "ensure the previous borrow is no longer in use"
- (empty vec in many cases)

All prose. No span for the fix, no replacement string, no
applicability marker.

### 2.2 The LSP loses suggestions entirely

`crates/riven-ide/src/diagnostics.rs:30-60`:

```rust
pub fn borrow_error_to_lsp(err, line_index, uri) -> LspDiagnostic {
    // ... builds LspDiagnostic with related_information from secondary
    // err.help is NEVER READ
}
```

Consequence: even if a suggestion is machine-applicable, the editor
can't auto-apply it. The user sees the terminal help but not a Quick
Fix button.

### 2.3 `Diagnostic` top-level carrier

`crates/riven-core/src/diagnostics/mod.rs:21-27`:

```rust
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    pub span: Span,
    pub code: Option<String>,
}
```

No help field at all. `BorrowError` has help; `Diagnostic` does not.

---

## 3. Goals & non-goals

### 3.1 Goals

- A structured `Suggestion` type: span(s) + replacement text +
  applicability level + short description.
- `Diagnostic` carries `suggestions: Vec<Suggestion>`.
- Terminal rendering: `help: …` with "↓" arrows at the relevant spans.
- LSP integration: `textDocument/codeAction` returns every suggestion
  as a code action, with confidence → `kind` mapping.
- `riven fix` (tier5_02) applies every `MachineApplicable`
  suggestion and flags the rest.
- A small builder API so compiler passes can emit suggestions without
  ceremony.

### 3.2 Non-goals

- Multi-file suggestions (insert a new file). Defer.
- Suggestions with live preview / diff UI at compile time. That's
  editor-side.
- AI-generated suggestions. This is purely mechanical, span-based.
- Full type-directed "did you mean" search across the whole crate. A
  dedicated typo corrector (Levenshtein on names in scope) is a
  downstream consumer; the suggestion framework just transports the
  result.

---

## 4. Surface

### 4.1 `Suggestion` type

```rust
// crates/riven-core/src/diagnostics/suggestion.rs

/// A span-based, potentially-multi-part replacement.
///
/// One `Suggestion` represents one logical fix. A fix may consist of
/// multiple edits (e.g. add `mut` in a declaration AND change a caller).
#[derive(Debug, Clone)]
pub struct Suggestion {
    /// Human-readable description ("consider borrowing here").
    pub message: String,
    /// The edits that make up this fix.
    pub edits: Vec<SuggestionEdit>,
    /// How confident we are that applying the edit yields a
    /// compiling program with the user's intended meaning.
    pub applicability: Applicability,
}

#[derive(Debug, Clone)]
pub struct SuggestionEdit {
    /// The span to replace (INCLUSIVE of start, EXCLUSIVE of end;
    /// same convention as `Span`).
    pub span: Span,
    /// The text to substitute for that span.
    pub replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Applicability {
    /// The edit compiles and is almost certainly what the user
    /// wanted. Tools MAY apply it without confirmation.
    MachineApplicable,

    /// The edit compiles, but may not match intent. Tools MUST
    /// prompt the user before applying.
    MaybeIncorrect,

    /// The edit contains placeholders (e.g. `{expr}`, `TODO`).
    /// Tools MUST surface to the user and NOT auto-apply.
    HasPlaceholders,
}
```

### 4.2 Why three levels, not five (as in rustc)?

Rust has `MachineApplicable`, `MaybeIncorrect`, `HasPlaceholders`,
and `Unspecified`. We drop `Unspecified`:

- `Unspecified` exists in Rust for compat with old suggestions that
  didn't declare. We are starting fresh — every suggestion declares a
  level.
- Requiring an explicit level forces the emitter to think about
  whether their fix is safe.

We keep the other three. `HasPlaceholders` is needed because some
suggestions literally contain prose (e.g. "consider adding an
explicit type annotation" with a `<type>` placeholder) — the editor
must present a form.

### 4.3 Diagnostic carrier update

Extend the new unified `Diagnostic` (tier5_04 §4.5):

```rust
pub struct Diagnostic {
    pub code: Option<ErrorCode>,
    pub level: DiagnosticLevel,
    pub primary: Label,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,       // free-form "note:" lines
    pub help: Vec<String>,        // free-form "help:" lines (no span)
    pub suggestions: Vec<Suggestion>,  // NEW: structured help
}
```

Two help channels intentionally:

- `help: Vec<String>` for prose that has no span or replacement.
  ("the borrow occurs because of Rust's lifetime rules; see…")
- `suggestions: Vec<Suggestion>` for actionable, span-based fixes.

### 4.4 Builder API

```rust
impl Diagnostic {
    pub fn error(code: ErrorCode, primary: Label) -> Self { ... }

    pub fn with_secondary(mut self, label: Label) -> Self { ... }
    pub fn with_note(mut self, note: impl Into<String>) -> Self { ... }
    pub fn with_help(mut self, help: impl Into<String>) -> Self { ... }

    pub fn with_suggestion(mut self, s: Suggestion) -> Self { ... }
    pub fn suggest_replace(
        mut self,
        span: Span,
        replacement: impl Into<String>,
        message: impl Into<String>,
        applicability: Applicability,
    ) -> Self {
        self.suggestions.push(Suggestion {
            message: message.into(),
            edits: vec![SuggestionEdit { span, replacement: replacement.into() }],
            applicability,
        });
        self
    }

    pub fn suggest_insert_before(
        self,
        span: Span,
        text: impl Into<String>,
        message: impl Into<String>,
        applicability: Applicability,
    ) -> Self { /* span.end == span.start at insert point */ }

    pub fn suggest_insert_after(...) -> Self { ... }

    pub fn suggest_remove(span: Span, message, appl) -> Self {
        self.suggest_replace(span, "", message, appl)
    }
}
```

Common patterns become one-liners:

```rust
Diagnostic::error(E1007, primary_label)
    .with_secondary(decl_label)
    .suggest_insert_after(
        let_span,
        "mut ",
        "consider declaring with `let mut {name}`",
        Applicability::MachineApplicable,
    )
```

### 4.5 Terminal rendering

Example output:

```
error[E1001]: value used after move
  --> src/main.rvn:9:8
   |
 7 | let name = String.from("Riven")
   |     ---- value created here
 8 | consume(name)
   |         ---- value given to `consume()` here
 9 |   puts name
   |        ^^^^ `name` used here after move
   |
help: consider cloning the value
   |
 8 | consume(name.clone)
   |             ++++++
   |
help: or borrow
   |
 8 | consume(&name)
   |         +
   |
   = note: see `riven explain E1001` for details
```

Each `Suggestion` gets its own `help:` block with a rendered mini-diff
(just the changed line with `+` / `-` / carets). The renderer
reuses the primary diagnostic's span to fetch the source line, then
applies the edit to show the fixed version.

Multi-edit suggestions show all edits in the same block.

### 4.6 LSP mapping

`crates/riven-ide/src/diagnostics.rs` gets a new function:

```rust
pub fn diagnostic_to_lsp(
    diag: &Diagnostic,
    line_index: &LineIndex,
    uri: &Url,
) -> (LspDiagnostic, Vec<CodeAction>) { ... }
```

Returns both the diagnostic (as today) *and* a list of code actions.
Each `Suggestion` becomes a `CodeAction`:

```rust
CodeAction {
    title: suggestion.message,
    kind: Some(match suggestion.applicability {
        MachineApplicable => CodeActionKind::QUICKFIX,
        MaybeIncorrect    => CodeActionKind::QUICKFIX,  // but client may
                                                          // surface differently
        HasPlaceholders   => CodeActionKind::REFACTOR,
    }),
    diagnostics: Some(vec![diag.clone()]),
    edit: Some(WorkspaceEdit { /* text edits from suggestion.edits */ }),
    is_preferred: Some(suggestion.applicability == MachineApplicable),
    ...
}
```

The LSP server's `codeAction` handler collects all current diagnostics
at the cursor's range and returns their code actions.

**Important:** `isPreferred: true` is what drives editors to
auto-apply with `Cmd-.`. Only `MachineApplicable` gets it.

### 4.7 JSON output

`rivenc --error-format=json` emits diagnostics as JSON, including
suggestions:

```json
{
  "code": "E1001",
  "level": "error",
  "message": "value used after move",
  "primary": { "span": {...}, "message": "..." },
  "secondary": [...],
  "notes": [...],
  "help": [...],
  "suggestions": [
    {
      "message": "consider cloning the value",
      "applicability": "machine-applicable",
      "edits": [
        { "span": {...}, "replacement": ".clone" }
      ]
    }
  ]
}
```

This is what `riven fix` consumes. External tools (VSCode extensions
not using the LSP, linters, CI reporters) consume this.

---

## 5. Architecture / design

### 5.1 File organization

New:
- `crates/riven-core/src/diagnostics/suggestion.rs` — `Suggestion`,
  `SuggestionEdit`, `Applicability`.
- `crates/riven-core/src/diagnostics/render.rs` — terminal renderer
  (multi-line, with source context). Moves the `Display` impl off the
  carrier.
- `crates/riven-core/src/diagnostics/json.rs` — JSON serialization
  behind a `json-output` feature flag.

Modified:
- `crates/riven-core/src/diagnostics/mod.rs` — `Diagnostic` carrier
  gains `suggestions`, `secondary`, `notes`, `help`. Builder methods.
- `crates/riven-ide/src/diagnostics.rs` — rewrite entirely once the
  carriers merge; add code-action generation.
- `crates/riven-lsp/src/server.rs` — handle `textDocument/codeAction`
  by delegating to `riven-ide`.
- Every existing `BorrowError` site in `borrow_check/mod.rs` (~15
  spots) — rewritten as `Diagnostic::error(code, primary).with_*()`
  chains.

### 5.2 Applicability policy for the compiler

Guidelines for the compiler's own code:

- **MachineApplicable** ONLY when the emitter is confident the
  replacement yields a compiling, semantically-correct program, e.g.:
  - Adding `mut` to a `let` declaration (`E1007`).
  - Adding `&` before an expression to make a borrow (`E1001` on
    some sites).
  - Renaming a typo to a known candidate with Levenshtein distance ≤ 1.
  - Edition-migration rewrites (`Hash[K, V]` → `HashMap[K, V]`).
- **MaybeIncorrect** when the replacement compiles but the user's
  intent is ambiguous, e.g.:
  - Suggesting `.clone()` for a move error (the user might have
    wanted a borrow instead).
  - Suggesting `expect("…")` when `?` might be better.
- **HasPlaceholders** when the replacement contains `_`, `TODO`, or
  typed-hole text the user must fill in, e.g.:
  - "add a type annotation here: `: <type>`".
  - "provide the missing variant: `| Variant(...) => TODO`".

### 5.3 Multi-edit suggestions

`SuggestionEdit::span` convention: all edits in a single `Suggestion`
are applied atomically, in order of `span.start`. Overlapping spans
within a single suggestion are a bug (asserted in debug builds).

A suggestion can cross non-contiguous regions of the file — e.g. add
a `mut` at the let-binding AND change every `&x` to `&mut x`. This is
one user-visible fix; reusing atomic semantics keeps `riven fix`
simple.

### 5.4 Overlap between suggestions

If two `Suggestion`s on the same `Diagnostic` touch overlapping spans,
`riven fix` applies at most one — the first `MachineApplicable` in
declaration order. The others become `MaybeIncorrect` for the tool's
purposes.

### 5.5 Validation

Debug-only assertions:

- Every `SuggestionEdit.span` is within the source file.
- Edits in one `Suggestion` are non-overlapping.
- If `Applicability::MachineApplicable`, the replacement does not
  contain placeholders like `<…>`, `{…}`, `TODO` (heuristic — not
  bulletproof, but catches obvious mistakes).

### 5.6 Serialization stability

The JSON format (§4.7) is part of the compiler's public API once
shipped. Tools will build on it. Versioning:

```json
{
  "riven_diagnostic_version": 1,
  "diagnostics": [ ... ]
}
```

Breaking changes bump the version; tooling pins.

---

## 6. Implementation plan

### 6.1 Phase 5a — Suggestion type + builder (1 week)

1. Create `diagnostics/suggestion.rs`.
2. Extend `Diagnostic` carrier with `suggestions: Vec<Suggestion>`,
   `secondary: Vec<Label>`, `notes: Vec<String>`, `help: Vec<String>`.
3. Builder API (§4.4).
4. Unit tests: construction, applicability levels, validation.

### 6.2 Phase 5b — terminal renderer (1-2 weeks)

1. `diagnostics/render.rs` with a full rustc-alike multi-line
   renderer. Source-line fetching, caret underlining, `help:` blocks
   with mini-diffs.
2. Update `fmt::Display for Diagnostic` to call the renderer.
3. Replace `BorrowError::fmt` (`errors.rs:64-77`) with renderer
   delegation.
4. Output fixtures in `tests/diagnostic_rendering/` — snapshot tests
   (input `.rvn`, expected rendered-output `.txt`).

### 6.3 Phase 5c — compiler-wide suggestion emission (2-3 weeks)

Pick high-value sites first. Concrete targets (each is one PR):

1. **E1001 use-after-move** — "consider cloning:"
   `MaybeIncorrect` (cloning is a semantic choice); "consider
   borrowing:" `MaybeIncorrect`.
2. **E1006 assign-to-immutable** — "consider declaring with
   `let mut`:" `MachineApplicable`. Edit: insert `mut ` at let-span.
3. **E1007 mut-borrow of immutable** — same as E1006.
4. **Typo / unresolved-name (E0300-ish)** — Levenshtein candidate
   search over symbols in scope. `MachineApplicable` for distance ≤ 1,
   else `MaybeIncorrect`.
5. **Private-item access (E0603-ish)** — "make `X` public:"
   `MaybeIncorrect`. Edit: insert `pub ` before the `def`/`class`
   keyword at the target's definition span.
6. **Type mismatch (E0500-ish)** — if the mismatch is `T` vs `&T`,
   "consider borrowing:" `MachineApplicable`. If it's `T` vs
   `Option[T]`, "consider wrapping with `Some(...)`:"
   `MaybeIncorrect`.

Each suggestion is one PR with a fixture.

### 6.4 Phase 5d — LSP code actions (1 week)

1. `riven-ide/src/diagnostics.rs` returns code actions.
2. `riven-lsp/src/server.rs` implements
   `textDocument/codeAction`.
3. VSCode integration-test (manual): write a program with E1006, hit
   `Cmd-.`, assert `let mut` fix applies.

### 6.5 Phase 5e — JSON output + `riven fix` (2 weeks)

1. `rivenc --error-format=json` (also shared with tier5_04 phase 4e).
2. `crates/riven-cli/src/fix.rs` — consumes JSON, applies
   machine-applicable edits.
3. Integration test: fixture crate with several E1006-type errors,
   `riven fix` rewrites and the crate compiles.

---

## 7. Interactions with other tiers

- **Tier 5 doc 02 (editions):** `riven fix --edition=YYYY` is the
  primary consumer. Edition migration is "apply all
  MachineApplicable suggestions and re-compile."
- **Tier 5 doc 03 (attributes):** deprecation warnings may emit
  suggestions (e.g. `@[deprecated(note = "use `new`")]` → "replace
  `old_name` with `new_name`", `MaybeIncorrect` by default because
  we don't know call-site context).
- **Tier 5 doc 04 (error codes):** the `Diagnostic` carrier is shared;
  the JSON format emits both together.
- **Tier 3 LSP:** this doc is effectively the LSP story for code
  actions. The LSP pipeline already handles diagnostics; code
  actions are the remaining big win.
- **Tier 1 B2 / derive:** the derive pipeline's errors (e.g. "Copy
  requires Clone") get machine-applicable suggestions: "add `Clone`
  to the derive list."
- **Tier 2 (parser macros, if any):** macro-generated suggestions
  must carry the expansion context. Deferred.

---

## 8. Phasing

| Phase | Work | Weeks | Gate |
|-------|------|-------|------|
| 5a    | `Suggestion` type + builder | 1 | All downstream |
| 5b    | Terminal renderer | 1-2 | User-visible quality |
| 5c    | High-value suggestion emissions (6 sites) | 2-3 | Real UX win |
| 5d    | LSP code actions | 1 | Editor UX |
| 5e    | JSON + `riven fix` | 2 | Migrator + CI tooling |

Total: ~7-9 weeks. Phases 5a and 5b can ship behind 5c's actual
content.

---

## 9. Open questions & risks

### OQ-1. Three applicability levels enough?

**Recommended:** yes (§4.2). Start minimal. If downstream tooling
demands finer distinctions, we can split a level without breaking
existing consumers (the stricter interpretation stays compatible).

### OQ-2. Multi-file suggestions?

Out of scope for v1. A suggestion spans one file. Future work: allow
`WorkspaceEdit`-style multi-file suggestions for refactorings like
"move this type to its own module."

### OQ-3. Stable span encoding in JSON output?

Today `Span` is `{start: u32, end: u32, line: u32, column: u32}`.
Byte offsets are version-sensitive if the source changes. The LSP
path uses `Position { line, character }` which is stable per file
version. **Recommended:** JSON format emits both byte-offset (for
compiler-internal tooling) and line/column (for editors). Include a
`source_hash: "sha256:..."` at the top level so consumers can detect
stale output.

### OQ-4. Self-testing of suggestions?

Risk: a suggestion ships claiming `MachineApplicable`, but applying it
produces code that doesn't compile. Mitigation:

- Regression tests: each suggestion fixture has an "after" state;
  the test applies the edit and asserts compilation.
- Dogfooding: `riven fix` run on the compiler's own crates during
  CI.

### OQ-5. What if the source has already changed between diagnostic
      emission and edit application?

The LSP handles this via document versioning (`textDocument.version`).
CLI tools re-run the compiler and re-compute. The suggestion's span
is stale once the source mutates. `riven fix` operates on the
compiler's just-emitted JSON — no staleness.

### OQ-6. Confidence level "auto" based on heuristics?

Tempting: pattern-match the message and infer applicability. Too
fragile. **Recommended:** every emission site explicitly states its
level. Compiler-internal lint checks that nobody uses a
placeholder without HasPlaceholders.

### OQ-7. Risk: the builder API encourages prose that doesn't fit on
      one line.

Mitigation: a debug-build lint warns if `Suggestion.message` is over
~60 chars or contains a newline. `help:` can be multi-line; a single
suggestion message should not.

### OQ-8. Duplicate/overlapping suggestions across multiple
      diagnostics.

If two diagnostics at different spans both suggest "add `mut` at line
3", `riven fix` applies the edit once (dedup by span+replacement). If
two suggestions conflict (same span, different replacements), only
the first `MachineApplicable` wins; the rest log.

### OQ-9. Risk: encouraging the compiler to ship `MaybeIncorrect`
      suggestions that are almost-never-right noise.

Heuristic: if a suggestion has applicability `MaybeIncorrect` but
multiple alternatives, suppress the rendering and emit only prose
("help: consider `clone`, `copy`, or borrowing"). Editors still see
the suggestions via JSON and can offer them; terminal output stays
readable.

### OQ-10. Should the Diagnostic carrier include a
       `requires_feature: Option<String>` field (for unstable-gated
       suggestions)?

Probably future work. For now, a suggestion targeting a nightly-only
construct simply wouldn't be emitted on stable.

---

## 10. Acceptance criteria

- [ ] `Suggestion`, `SuggestionEdit`, `Applicability` exist in
      `crates/riven-core/src/diagnostics/suggestion.rs`.
- [ ] `Diagnostic.suggestions: Vec<Suggestion>` is the canonical
      carrier.
- [ ] Builder API (`suggest_replace`, `suggest_insert_before`,
      `suggest_remove`, `with_suggestion`) works and is exercised by
      test fixtures.
- [ ] Terminal rendering shows each suggestion as a `help:` block
      with mini-diff of the edit.
- [ ] `BorrowError.help: Vec<String>` is gone or reimplemented on
      `Diagnostic`.
- [ ] At least the six high-value suggestion call sites in §6.3 are
      live, with snapshot tests.
- [ ] LSP `textDocument/codeAction` returns code actions for every
      suggestion.
- [ ] `MachineApplicable` suggestions carry `isPreferred: true`.
- [ ] `rivenc --error-format=json` emits suggestions in the
      JSON schema from §4.7.
- [ ] `riven fix` applies all `MachineApplicable` suggestions in a
      fixture crate and the result compiles.
- [ ] Snapshot regressions: every suggestion fixture has before+after,
      the after compiles.
- [ ] Debug-build assertions validate span bounds and detect
      placeholders in `MachineApplicable` suggestions.
