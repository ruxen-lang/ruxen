# Tier 5 — Error-Code Registry + `--explain`

Status: draft
Depends on: tier5_03 phase 3a (warning-level attribute support — for
`@[allow(...)]`), tier5_05 (suggestion struct — shares the Diagnostic
carrier).
Blocks: credible borrow-check UX. Rust's biggest UX win was `rustc
--explain E0308`; Riven users will hit borrow-check errors harder than
Rust users (smaller surface, less Stack Overflow).

---

## 1. Summary & motivation

Error messages are a user's first experience with a language's
difficulty. Riven's core value proposition — "Rust safety, Ruby
expressiveness" — puts the borrow checker front and centre. A user who
can't read `error[E1001]: use after move` will give up.

Rust's solution is a **registry** of error codes (`E0308`, `E0277`, …,
~1000 total) with **long-form explanations** fetched via `rustc
--explain E0308`. A user hitting a confusing error can run one command
and get a paragraph-long explanation with examples of bad code and
fixed code.

Riven has the skeleton of this already:

- `ErrorCode::E1001..E1010` for borrow check (`borrow_check/errors.rs:
  6-16`).
- `Diagnostic.code: Option<String>` + `error_with_code` (`diagnostics/
  mod.rs:26, 39`).
- `BorrowError` renders `error[E1001]: use after move` (`errors.rs:66`).

What's missing:

- No `riven --explain EXXXX` subcommand.
- No registry; codes live in scattered enums per-subsystem.
- No long-form explanations.
- Most diagnostics don't have codes: lexer, parser, resolver, typeck,
  MIR — `grep -r error_with_code crates/riven-core` returns zero non-
  test hits.

This doc specifies the code-namespace policy, the registration
mechanism, the explanation-file format, the `--explain` subcommand, and
how to roll out codes across the compiler.

---

## 2. Current state

### 2.1 Existing error code enum

`crates/riven-core/src/borrow_check/errors.rs:5-48`:

```rust
pub enum ErrorCode {
    E1001, // use after move
    E1002, // can't mut-borrow while immutably borrowed
    E1003, // can't immut-borrow while mutably borrowed
    E1004, // can't move out of borrowed reference
    E1005, // borrow outlives owner
    E1006, // assign to immutable variable
    E1007, // can't mut-borrow immutable variable
    E1008, // value moved into closure
    E1009, // can't move while borrowed
    E1010, // returned reference outlives local
}

impl ErrorCode {
    pub fn title(&self) -> &'static str { ... }
    pub fn code_str(&self) -> &'static str { ... }
}
```

Tests (`borrow_check/tests.rs:135, 315, ...`) reference specific
variants.

### 2.2 Diagnostic carrier (top-level)

`crates/riven-core/src/diagnostics/mod.rs:21-56`:

```rust
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    pub span: Span,
    pub code: Option<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, span: Span) -> Self { ... }
    pub fn error_with_code(message, span, code) -> Self { ... }
    pub fn warning(message: impl Into<String>, span: Span) -> Self { ... }
}
```

`BorrowError` is a **separate** struct (not a `Diagnostic`)
with its own `Display` impl (`borrow_check/errors.rs:64-77`). They
render differently. The IDE glue at `crates/riven-ide/src/diagnostics.
rs:30-60` has two different conversion functions.

### 2.3 Reserved codes from tier-1 docs

| Range | Reserved by | Purpose |
|-------|-------------|---------|
| `E1001-E1010` | Live today | Borrow check (`borrow_check/errors.rs`) |
| `E1011-E1016` | tier1_02_concurrency.md:785-790 | Send/Sync, mutex, etc. |
| `E0601-E0609` | tier1_05_derive_macros.md:718 | Derive |

### 2.4 What's absent

- No `riven --explain X` in `crates/rivenc/src/main.rs:40-68` nor
  `crates/riven-cli/src/cli.rs:25-113`.
- No `docs/errors/` directory.
- No central `ErrorCode` enum spanning the whole compiler. Per-
  subsystem enums will multiply (doc 05 adds more, tier1 adds more).
- Lexer errors, parser errors, typeck errors — **none** use
  `error_with_code`. Every `self.error("expected ...")` is coded-less.

---

## 3. Goals & non-goals

### 3.1 Goals

- A **single, crate-wide** `ErrorCode` enum in `riven-core` naming
  every error code in the compiler.
- Every error-level `Diagnostic` emitted by the compiler carries an
  `ErrorCode` (no more bare `.error(msg, span)` in end-user output).
- A **namespacing scheme** that leaves room for 30+ years of growth.
- `docs/errors/E????.md` — one file per code, with a structured format.
- `riven --explain EXXXX` subcommand. Mirror on `rivenc --explain` for
  users who only have the compiler.
- A **CI lint** that every `ErrorCode` variant has a `.md` file, and
  every `.md` file corresponds to a live variant.
- Warning codes (`W????`) follow the same policy; `W2001`-etc. from
  tier5_03.
- Stable: once published, an `ErrorCode` variant's *number* doesn't
  change. The message, explanation, and set of triggering conditions
  may change with a release note.

### 3.2 Non-goals

- Per-language-variant error messages (i18n). Defer.
- Exhaustive explanation prose at launch. Ship `.md` stubs; write
  prose incrementally.
- Programmatic "error taxonomy" with tags (`#borrow-check`, `#type-
  inference`, …). Nice-to-have; defer to a v2 of the registry.
- Error auto-bisection / "run this command to reproduce." Different
  problem.

---

## 4. Surface

### 4.1 Code namespace policy

Number ranges (repeated from overview §7.2 for convenience):

| Range | Domain |
|-------|--------|
| `E0001-E0099` | General / lexer |
| `E0100-E0299` | Parser (syntax) |
| `E0300-E0499` | Name resolution / imports / visibility |
| `E0500-E0899` | Type check / inference / coercion |
| `E0900-E0999` | Trait resolution |
| `E1000-E1999` | Borrow check / lifetimes / ownership |
| `E2000-E2999` | Attributes, stability, features, lints |
| `E3000-E3999` | MIR / const-eval / unreachable |
| `E4000-E4999` | Linking / codegen / manifest / editions |
| `E5000-E5999` | Build tool / package manager |
| `E9000-E9999` | Unassigned / experimental / stub |

Warnings: `W` prefix, same numeric ranges by domain.

**Rationale:** Rust's `E0001-E0800` block was filled in over a decade.
We leave ample room and domain-grouped numbers so contributors know
where to add a new variant.

**Numbers are stable** once published. A code may be retired
("`E0527` was only emitted on edition 2026; 2028 compiler never
produces it") but its slot is not reused.

### 4.2 Code format

Five characters: letter + four digits. Always four digits with leading
zeros (`E0001`, `E0308`, `E1001`). Padded so sorts are natural and
`grep 'E\d\d\d\d'` matches cleanly.

### 4.3 Registry module

New module `crates/riven-core/src/error_codes.rs`:

```rust
//! Central registry of all error codes emitted by the Riven compiler.
//! Every code is listed here, even if only one subsystem emits it.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    // ── Lexer / general (E0001-E0099) ──
    E0001,  // unterminated string literal
    E0002,  // invalid escape sequence
    // ...

    // ── Parser (E0100-E0299) ──
    E0100,  // expected expression
    E0101,  // expected `end` keyword
    // ...

    // ── Resolve (E0300-E0499) ──
    E0300,  // unresolved identifier
    E0301,  // duplicate definition
    // ...

    // ── Type check (E0500-E0899) ──
    E0500,  // type mismatch
    E0501,  // cannot infer type
    // ...

    // ── Trait resolution (E0900-E0999) ──
    E0900,  // trait not implemented
    // ...

    // ── Borrow check (E1001-E1999) ──
    E1001,  // use after move
    E1002,  // cannot borrow as mutable — already borrowed as immutable
    // ... (existing)
    E1011,  // value not Send (from tier1_02)
    E1012,  // value not Sync
    E1013,  // non-'static capture in spawn
    // ...

    // ── Attributes / stability (E2000-E2999) ──
    E2001,  // attribute arg expected
    E2002,  // unstable feature without opt-in (from tier5_03)
    // ...
    W2001,  // deprecated-use warning (warning-level; we include in the enum)
    // ... (see §4.4 on whether to split enums)

    // ── MIR (E3000-E3999) ──
    E3000,  // unreachable code after return
    // ...

    // ── Codegen / linking / editions (E4000-E4999) ──
    E4100,  // unknown edition (from tier5_02)
    E4101,  // crate requires newer compiler
    // ...

    // ── Build tool (E5000-E5999) ──
    E5001,  // Riven.toml not found
    // ...
}

impl ErrorCode {
    /// Five-character representation: "E0308", "E1001", etc.
    pub fn as_str(&self) -> &'static str { ... }

    /// One-line title, matching the current `BorrowError.title()` pattern.
    pub fn title(&self) -> &'static str { ... }

    /// Long-form explanation, loaded from `docs/errors/E????.md`.
    /// None if the code has no explanation yet.
    pub fn explain(&self) -> Option<&'static str> {
        match self {
            ErrorCode::E1001 => Some(include_str!("../../../docs/errors/E1001.md")),
            // ...
        }
    }

    /// All known codes. Used by `riven --explain` and the registry-coverage test.
    pub fn all() -> &'static [ErrorCode] { ... }
}
```

Generation of `all()`, `as_str()`, `title()`, `explain()` is
boilerplate. Use a `macro_rules!` or a `build.rs` + `include!` to keep
it DRY. `build.rs` can also fail the build if a variant has no `.md`
file.

### 4.4 Warnings vs errors in the same enum?

**Recommended:** **single enum**, with `W` codes as variants. The
carrier type already holds `level` (`DiagnosticLevel::Error` vs
`Warning`) — the code's letter prefix is informational. Keeping them
in one enum means:

- One registry.
- One lint-level map.
- Simpler tooling (`riven --explain W2001` works the same as
  `--explain E1001`).

Alternative: split `ErrorCode` / `WarningCode`. Rejected — small
duplication for marginal safety.

### 4.5 Diagnostic unification

**This is a prerequisite.** `BorrowError` and `Diagnostic` should
merge. Proposed new carrier in `crates/riven-core/src/diagnostics/
mod.rs`:

```rust
pub struct Diagnostic {
    pub code: Option<ErrorCode>,          // was Option<String>
    pub level: DiagnosticLevel,
    pub primary: Label,                   // was `span: Span` + `message: String`
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Vec<String>,
    pub suggestions: Vec<Suggestion>,     // defined in tier5_05
}

pub struct Label {
    pub span: Span,
    pub message: String,
}
```

`BorrowError` becomes a factory function returning `Diagnostic`:

```rust
pub fn borrow_diag(code: ErrorCode, primary: Label, secondary: Vec<Label>,
                   help: Vec<String>) -> Diagnostic { ... }
```

### 4.6 Explanation file format

`docs/errors/E1001.md`:

```markdown
# E1001: value used after move

Non-`Copy` types in Riven have a single owner at a time. Assigning or
passing a value transfers ownership; the original binding becomes
unusable.

## Erroneous example

```rvn
def consume(s: String)
  puts s
end

def main()
  let name = String.from("Riven")
  consume(name)   # `name` moved into `consume`
  puts name        # ERROR: `name` was moved on the previous line
end
```

## Why this is an error

Allowing reads of moved values would risk reading freed memory or
duplicated references to the same resource. Riven makes this error
loud so you catch it at compile time.

## How to fix

There are three standard fixes depending on intent:

### 1. Borrow instead of moving (the usual fix)

```rvn
def consume(s: &String)     # borrow, not own
  puts s
end

let name = String.from("Riven")
consume(&name)               # pass a borrow
puts name                    # still valid
```

### 2. Clone before the transfer (when you truly need two copies)

```rvn
consume(name.clone)
puts name
```

### 3. Reinitialize the binding

```rvn
let mut name = String.from("Riven")
consume(name)
name = String.from("reused")  # new value in the same variable
puts name
```

## See also

- [Ownership and borrowing (reference)](/docs/reference/06-semantics/02-ownership-and-move.md)
- E1008: value moved into closure
- E1009: cannot move while borrowed
```

Sections (normative for authors):

1. **Title line** — `# EXXXX: <title>` — must match
   `ErrorCode::title()` byte-for-byte.
2. **One-paragraph summary.**
3. **Erroneous example** — fenced `rvn` block the compiler would
   actually reject. A test harness compiles it and asserts the rejection.
4. **Why this is an error.**
5. **How to fix** — one or more subsections with fixed code.
6. **See also.**

Every section is informative except the title (which is normative-
linked to `ErrorCode::title`).

### 4.7 `--explain` subcommand

Usage:

```
riven explain EXXXX             # preferred form (`riven` is the user-facing tool)
rivenc --explain EXXXX          # also supported (the compiler's flag-style)
rivenc --explain=EXXXX          # equivalent
```

Behaviour:

- Print the Markdown (unrendered) to stdout.
- If stdout is a TTY: try to pipe through `$PAGER` (default: `less -R`).
- Unknown code: `error: no error code 'EXXXX' is known by this compiler;
  run \`riven explain --list\` for all known codes`.

`riven explain --list`:

- Print a table of `EXXXX  title`, sorted.
- Useful for spelunking.

`riven explain --list --json`:

- JSON array of `{code, title, domain, kind: "error"|"warning"}`. Used
  by external tools / documentation generators.

### 4.8 Diagnostic rendering update

`Diagnostic.fmt` currently produces a single-line format (`diagnostics/
mod.rs:58-70`). Upgrade to a rustc-alike multi-line format:

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
   = help: consider cloning the value: `name.clone`
   = help: or borrow: `consume(&name)`
   = note: see `riven explain E1001` for details
```

The `BorrowError` rendering (`errors.rs:64-77`) is already in this
shape; we generalize it to all `Diagnostic` values.

---

## 5. Architecture / design

### 5.1 Where `ErrorCode` lives

`crates/riven-core/src/error_codes.rs` — new module.

`crates/riven-core/src/diagnostics/mod.rs` — imports `ErrorCode`, uses
in `Diagnostic.code: Option<ErrorCode>`.

`crates/riven-core/src/borrow_check/errors.rs` — the local
`ErrorCode` enum is deleted; replaced with re-export:
`pub use crate::error_codes::ErrorCode;`. All existing call sites keep
compiling.

### 5.2 `docs/errors/` layout

```
docs/errors/
├── README.md            # explains the format, links to the registry module
├── E0001.md
├── E0002.md
├── ...
├── E1001.md             # already the first code to get a full write-up
├── ...
├── W2001.md
└── _template.md         # copy-paste skeleton
```

### 5.3 Build-time inclusion

`build.rs` in `riven-core` scans `docs/errors/*.md`, fails the build if
any file-to-variant or variant-to-file mismatch, and emits an
`include!()`-friendly `$OUT_DIR/error_explanations.rs`:

```rust
pub const EXPLANATIONS: &[(&str, &str)] = &[
    ("E0001", include_str!("..//docs/errors/E0001.md")),
    ...
];
```

`ErrorCode::explain` is then a table lookup.

Feature flag: `explain-embed` (default on). Turning it off excludes
the explanations from the binary (saves ~500 KB for a minimal
distributed compiler).

### 5.4 `riven explain` subcommand

`crates/riven-cli/src/cli.rs:25-113` — add:

```rust
Explain {
    /// Error or warning code, e.g. E1001
    code: Option<String>,
    /// List all known codes instead
    #[arg(long)]
    list: bool,
    /// Emit machine-readable output (with --list)
    #[arg(long)]
    json: bool,
},
```

Handler in `riven-cli/src/main.rs` calls into
`riven_core::error_codes::ErrorCode::explain(...)`.

For `rivenc` the flag is `--explain EXXXX`, parsed in
`crates/rivenc/src/main.rs:27-37`.

### 5.5 CI lint for registry coverage

Test `crates/riven-core/tests/error_code_registry.rs`:

```rust
#[test]
fn every_code_has_md() {
    for code in ErrorCode::all() {
        assert!(code.explain().is_some(),
                "{} is missing docs/errors/{}.md",
                code.as_str(), code.as_str());
    }
}

#[test]
fn every_md_has_variant() {
    let dir = Path::new("../../docs/errors/");
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if s.ends_with(".md") && s != "README.md" && !s.starts_with('_') {
            let code = s.strip_suffix(".md").unwrap();
            let parsed = ErrorCode::from_str(code)
                .unwrap_or_else(|_| panic!("docs/errors/{} has no variant", s));
            // ...
        }
    }
}

#[test]
fn title_matches_md_h1() {
    // Title line "# EXXXX: <title>" in the .md matches ErrorCode::title()
}
```

### 5.6 Populating codes — the migration

Running `grep 'diag.*error(' crates/riven-core/src` across the parser,
resolver, typeck, MIR, etc., turns up on the order of 300-400 call
sites. Each needs a code.

Approach:

1. Introduce the `ErrorCode` enum with a special variant `UNSPECIFIED`
   temporarily.
2. Add a `riven_core::diagnostics::error_unspecified` helper that
   emits `UNSPECIFIED`.
3. Do a blanket `sed` renaming every `error(msg, span)` call in end-
   user emission paths to `error_unspecified(msg, span)`.
4. Walk through each file, promoting `UNSPECIFIED` to a real code, one
   per semantic category.
5. When the count reaches zero: delete `UNSPECIFIED` and make the enum
   exhaustive.

This is mechanical and can be done incrementally across several PRs.

---

## 6. Implementation plan

### 6.1 Phase 4a — unified Diagnostic + ErrorCode enum (1-2 weeks)

1. Create `error_codes.rs` with the full ranges in §4.1. Start with
   the existing codes (`E1001-E1010`, plus tier-1 reserved) and a
   handful of representative new codes (`E0100` expected expression;
   `E0500` type mismatch; etc.).
2. Replace `Diagnostic.code: Option<String>` with
   `Option<ErrorCode>`. Update the LSP conversion in
   `riven-ide/src/diagnostics.rs:11-28`.
3. Migrate `BorrowError` to a factory over the new `Diagnostic`. Kill
   the `BorrowError` struct or keep it as a compat alias. Delete
   `riven-ide/src/diagnostics.rs:30-60` once merge is complete.
4. `Diagnostic` renderer in the rustc-alike multi-line shape (§4.8).

### 6.2 Phase 4b — `--explain` scaffolding (1 week)

1. `docs/errors/` with `_template.md`, `README.md`, and `.md` files
   for the ten existing `E1001-E1010` codes.
2. `build.rs` in `riven-core` generates `EXPLANATIONS` table.
3. `riven explain X` / `rivenc --explain X` subcommands.
4. Registry-coverage tests.

### 6.3 Phase 4c — compiler-wide code population (2-3 weeks)

1. Audit all `diag.push(Diagnostic::error(...))` sites.
2. Introduce temporary `UNSPECIFIED` variant.
3. PR-by-PR (each domain): lexer → parser → resolve → typeck → MIR →
   codegen. Each PR replaces `UNSPECIFIED` with specific codes.
4. At end: `UNSPECIFIED` deleted; every end-user error has a code.

This is by far the biggest phase; roughly 400 call sites, but purely
mechanical once the patterns land.

### 6.4 Phase 4d — explanation prose (ongoing)

Parallelisable across contributors. Each code gets a stub initially
("*explanation not yet written*"); prose filled in as contributors
volunteer. The registry-coverage test allows a stub `.md` but a second
test `every_code_has_nontrivial_explanation` warns (not errors) on
stub files, so a dashboard shows how many are still stubs.

### 6.5 Phase 4e — JSON output (small)

`rivenc --error-format=json` emits diagnostics as JSON (spans, codes,
suggestions). Consumed by external tooling, CI reporters, `riven fix`.
Already planned in tier5_05 §5.6; this phase is the joint deliverable.

---

## 7. Interactions with other tiers

- **Tier 5 doc 01 (reference):** `docs/reference/08-errors/index.md`
  links into `docs/errors/`.
- **Tier 5 doc 02 (editions):** reserved codes `E4100-E4199`. Edition
  deprecations use `W`-prefix codes from range `W2000-W2999`.
- **Tier 5 doc 03 (attributes):** `W2001` (deprecated use),
  `E2002` (unstable). Lint-level attributes (`@[allow(W2001)]`) can
  suppress by code too.
- **Tier 5 doc 05 (suggestions):** `Diagnostic.suggestions` field
  added in phase 4a.
- **Tier 3 (LSP):** `riven-ide/src/diagnostics.rs:22` already maps
  `diag.code` to `NumberOrString`. Once unified, this mapping drops
  the `BorrowError` branch.
- **Tier 1 concurrency (doc 02) / derive (doc 05):** both pre-
  reserved code ranges. Validate against this registry as they land.

---

## 8. Phasing

| Phase | Work | Weeks | Gate |
|-------|------|-------|------|
| 4a    | Unified Diagnostic + `ErrorCode` enum | 1-2 | Everything else |
| 4b    | `docs/errors/` + `--explain` + registry test | 1 | User-visible story |
| 4c    | Population across compiler | 2-3 | Most of the UX win |
| 4d    | Prose (rolling) | ongoing | Quality of UX |
| 4e    | `--error-format=json` | 1 | External tooling |

**Total for user-visible launch (phases 4a-4c):** ~4-6 weeks.

---

## 9. Open questions & risks

### OQ-1. Code granularity: one per variant, or grouped?

Rust uses one code per distinct error variant. **Recommended:** same
for Riven. One semantic error = one code. Two errors that happen to
share a title ("use after move" in two contexts) become separate codes
if the example / fix differ. Users search by code; ambiguous codes
waste their time.

### OQ-2. Should `--explain` live on `riven` or `rivenc` or both?

**Recommended:** both. `riven explain X` is the canonical user-facing
name (matches `riven build`, `riven run`). `rivenc --explain X` keeps
single-binary distributions (IDE bundles, CI images) self-contained.
Both delegate to the same `riven_core::error_codes::explain()`.

### OQ-3. `--explain` storage: embedded vs external?

**Recommended:** embedded (default), with a feature flag to strip for
minimal builds (§5.3). Rust's rustc embeds explanations. External
fetches (over HTTP) are unreliable for CI environments.

**Size estimate:** 1000 codes × ~2 KB per Markdown file ≈ 2 MB. Fine
for the default distribution; noteworthy for a stripped one.

### OQ-4. Code stability across major versions?

A code's **meaning** is stable. A code may be retired (no longer
emitted) but not repurposed. Its `.md` file stays in `docs/errors/`
with a "retired in version X.Y" header.

### OQ-5. Renumber the existing `E1001-E1010`?

They were assigned before this doc. They fit the scheme (borrow check
at `E1000+`). Keep as-is.

### OQ-6. Numeric collision risk: tier-1 docs reserved `E0601-E0609`
      (derive) — that's in the resolve/type-check range per §4.1.

`E0601-E0609` are actually consistent with tier5_04's scheme: they're
in the `E0500-E0899` type-check range (derive is a type-check concern
— "is this type eligible for `Copy`?"). **No renumber needed**, but
document the overlap: derive-specific codes live in the type-check
range, which is semantically correct.

### OQ-7. Warnings without codes?

Rust has unnamed lint-level diagnostics ("warning: unused variable").
**Recommended:** every warning Riven emits has a code (e.g. `W1000`
for unused variable). Lints are suppressible by code *or* by name
(`@[allow(unused)]`). Costs one code per lint; worth it for
consistency.

### OQ-8. Risk: `UNSPECIFIED` variant sticks around forever.

Mitigation: a CI test that fails on any `UNSPECIFIED` once phase 4c
completes. The variant itself is deleted at the end of 4c.

### OQ-9. Risk: `docs/errors/` bit-rots — titles drift, examples
      don't compile.

Mitigation:
- Test: `.md` `# EXXXX: title` matches `ErrorCode::title`.
- Test: every "Erroneous example" fence block is compiled and asserted
  to produce that code.
- Test: every "How to fix" block is compiled and asserted to succeed.
- This is substantial test machinery (`tests/error_explanations.rs`)
  but pays for itself every release.

### OQ-10. Multilingual `--explain`?

Deferred. `docs/errors/` is English. A future tier may add
`docs/errors/zh/E1001.md`, with `riven explain X --lang=zh`. Out of
scope for this doc.

### OQ-11. How does this interact with the existing
      `Diagnostic::error(message, span)` call — do we ban it?

Yes, eventually. Phase 4c renames every end-user call to
`error_with_code` (or the new builder pattern). Internal / temporary
/ panics keep `Diagnostic::error` during development. A lint flags
new uses after phase 4c.

---

## 10. Acceptance criteria

- [ ] `crates/riven-core/src/error_codes.rs` exists with a single
      `ErrorCode` enum.
- [ ] `Diagnostic.code: Option<ErrorCode>` replaces `Option<String>`.
- [ ] `BorrowError` is either deleted or re-implemented as a
      `Diagnostic` factory.
- [ ] `docs/errors/` exists with `_template.md` and at least the 10
      existing `E1001-E1010` explanations.
- [ ] `riven explain E1001` prints the Markdown from
      `docs/errors/E1001.md`.
- [ ] `rivenc --explain E1001` does the same.
- [ ] `riven explain --list` prints all known codes.
- [ ] `riven explain --list --json` prints a JSON table.
- [ ] Registry coverage test: every variant has a `.md` file; every
      `.md` file has a variant.
- [ ] Title-match test: `# EXXXX: title` in `.md` matches
      `ErrorCode::title()`.
- [ ] Every end-user diagnostic in `riven-core` has an `ErrorCode`.
- [ ] Multi-line rustc-alike rendering for all diagnostics.
- [ ] LSP `diag.code` field flows through (already works; verify no
      regressions).
- [ ] `rivenc --error-format=json` emits structured JSON.
