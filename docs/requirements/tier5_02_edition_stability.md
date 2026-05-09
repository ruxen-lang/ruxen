# Tier 5 — Edition / Stability Mechanism

Status: draft
Depends on: tier5_03 (attributes — editions use `@[unstable(feature = ...)]`),
tier5_05 (suggestions — machine-applicable suggestions drive migration).
Blocks: long-term language evolution. Every breaking change before the
edition mechanism is in place is either "too expensive to do" or "painful
when it lands."

---

## 1. Summary & motivation

`Riven.toml` already declares a package edition:

```toml
[package]
edition = "2026"
```

The field is parsed and stored (`crates/riven-cli/src/manifest.rs:28-29`),
the scaffolder emits it (`scaffold.rs:145`), and tests assert its presence
(`scaffold.rs:198`, `manifest.rs:329`). Every fixture crate declares
`edition = "2026"` (`tests/fixtures/*/Riven.toml:4`).

But the compiler never reads it. `grep edition` across `crates/riven-core`
and `crates/rivenc` turns up zero hits. There is no edition-gating, no
version-check, no semantic effect.

This is a ticking bug: every time we publish a library on the current
edition, we lock in *whatever the compiler did at that time*, with no way
to describe "this crate expects 2026 semantics." The first time we try to
make a cleanup change (e.g. tier-1 B3's proposed `Hash` → `HashMap`
rename), we'll either break everyone or shrug and accept a bad name.

This doc specifies:
- What editions mean for Riven (scope, cadence, lifespan).
- How the compiler consumes the `edition` field.
- What the migration tool (`riven fix --edition=…`) does and how it works.
- What can and cannot change between editions.

---

## 2. Current state

### 2.1 Manifest field

`crates/riven-cli/src/manifest.rs:23-47` (`Package` struct):

```rust
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub edition: Option<String>,
    ...
}
```

- Optional field; no validation that the value is a known edition string.
- Test at `manifest.rs:329` asserts round-trip but not policy.
- Scaffolder hardcodes `"2026"` at `scaffold.rs:145`.

### 2.2 MSRV field (distinct but related)

`Package::riven: Option<String>` (`manifest.rs:32`) — minimum supported
compiler version (e.g. `">=0.2.0"`). Also unused today. We will wire this
alongside editions because the two are linked (new editions usually
require a newer compiler).

### 2.3 Features / feature-gates: none

No `@[unstable(feature = "foo")]` exists. No `#[cfg(feature = "…")]`.
`BuildConfig` (`manifest.rs:77-91`) has `link` and `link-search` but no
`features` section. Cargo-style feature propagation is not modelled.

### 2.4 What edition-like behaviour Riven has de facto

- **Reserved keywords** (`lexer/token.rs:127-137`) — `actor`, `spawn`,
  `send`, `receive`, `macro`, `crate`, `extern`, `static`, `const`,
  `when`, `unless`. Reserved *today*, so introducing them as real
  keywords in a future version does not need an edition.
- **`derive`** (`token.rs:105`) — already a keyword; activating its
  semantics (tier1_05) is not an edition-breaking change because current
  usage errors out.

---

## 3. Goals & non-goals

### 3.1 Goals

- `package.edition = "2026"` has semantic effect.
- Old-edition code keeps compiling on new compilers. No silent breakage.
- Breaking changes are possible: via a new edition.
- **Cross-edition linking works.** A `"2026"` library can be consumed by
  a `"2027"` binary, and vice versa. (Rust's hard-won lesson; we inherit
  it.)
- A **migration tool** `riven fix --edition=N` rewrites source
  mechanically using machine-applicable suggestions (tier5_05).
- **Per-edition feature gates**: `@[unstable(feature = "foo")]` items
  are locked out unless the manifest opts in via `features = ["foo"]` or
  an equivalent.

### 3.2 Non-goals

- **Editions as a dumping ground for arbitrary breakage.** §4.2 constrains
  what editions may change.
- **Semver-style breakage *inside* an edition.** A crate with
  `edition = "2026"` must build on every 0.2.x compiler of the 2026 era.
- **Replacing MSRV.** `riven = ">=0.2.0"` remains orthogonal; it bounds
  which compiler versions work, not which syntax the source uses.
- **Go-style eternal backward compat with zero breakage.** See §9 OQ-1.

---

## 4. Surface

### 4.1 Manifest

```toml
[package]
name = "my-crate"
version = "0.1.0"
edition = "2026"           # one of a fixed known list
riven   = ">=0.3.0"        # MSRV; enforced by the compiler

[features]                 # NEW section
default = ["color"]
color   = []
parallel = ["rayon-like"]  # feature flags may pull in stdlib-unstable deps

[dependencies]
foo = { version = "1", features = ["extra"], default-features = false }
```

Validation:

- `edition` MUST be one of the strings in the `KNOWN_EDITIONS` table
  (compiled into the compiler; see §5.1). Unknown → `E4100: unknown
  edition "…"; this compiler knows "2026" and "2027"`.
- If absent, default is the compiler's **default edition** (not the
  latest; see §4.4).
- `riven = ">=…"` is compared to `CARGO_PKG_VERSION` of `rivenc`. If the
  manifest demands a newer compiler → `E4101: this crate requires
  Riven >=X.Y.Z; this compiler is vX.Y.W`.

### 4.2 What may change between editions

| Change kind | Allowed in an edition? |
|-------------|------------------------|
| New syntax (new keyword) | **Yes**, but the keyword must already be reserved in the older edition (see §4.3). |
| Deprecation of old syntax → warning | Yes (works for everyone, no edition needed). |
| Deprecation of old syntax → error | **Edition only.** Old edition keeps the warning; new edition promotes to error. |
| New semantic rule applying to existing syntax | **No.** E.g. changing `&String → &str` coercion to a different rule. |
| New method resolution preference | **No.** Method resolution order is cross-edition stable. |
| ABI / mangling changes | **No.** ABI is cross-edition stable (or explicitly opt-in via separate attribute, not edition). |
| Stdlib item rename/removal | **Yes**, new edition hides the old name. Cross-edition code calls it via a re-export shim. |
| Precedence change | **No.** Violates the principle that fixtures from the old edition parse the same way in the new compiler. |
| Lifetime-elision rule change | **No** (would silently change behaviour). |
| New reserved word | **Yes.** `lexer/token.rs:127-137` already does this via "reserved but unused" status. |

Principle: **editions may reshape surface syntax; they may not change
semantics of syntax that both editions accept.** If you need semantic
change, introduce a new syntax under an edition and migrate; the old
syntax keeps its old meaning.

### 4.3 Edition cadence

- **Default release cadence:** every 2-3 years, aligned with major
  language features maturing.
- **Current:** `"2026"` (the default today).
- **Next:** `"2027"` proposed — a minimum viable second edition to prove
  the machinery works.
- **Naming:** YYYY, not semver. Decouples from compiler version.

### 4.4 Edition support lifetime

- The compiler supports **the current edition + the previous edition** at
  any time. When a third edition arrives, the oldest is deprecated and
  then removed.
- Deprecation window: **one edition cycle** (≥ 2 years) between
  "compiler emits deprecation warning for this edition" and "compiler
  rejects this edition."
- `E4102: edition "2026" is deprecated; run `riven fix --edition=2028`
  to migrate`.

### 4.5 Feature gates (unstable features)

```riven
@[unstable(feature = "try_trait", issue = "42")]
pub trait Try
  ...
end
```

- Using an `@[unstable]` item requires opting in via manifest:
  ```toml
  [features]
  # nightly-like: must be named in the manifest explicitly
  unstable = ["try_trait"]
  ```
- On a **stable release** of the compiler, `unstable = […]` is rejected
  unless `RIVENC_ALLOW_UNSTABLE=1` is set (intentional friction, matches
  Rust's nightly channel pattern).
- On a **nightly / dev** compiler (future: add a channel to the compiler
  build), `unstable = […]` works transparently.
- An unstable feature graduates via an `@[stable(since = "0.5.0",
  feature = "try_trait")]` attribute when the design is settled.

Detailed attribute syntax lives in tier5_03.

### 4.6 The migrator: `riven fix`

New CLI subcommand on `riven` (`crates/riven-cli`):

```
riven fix [--edition=YYYY] [--dry-run] [--check]
```

- `--edition=YYYY`: upgrade to target edition.
- `--dry-run`: print the diff; don't write.
- `--check`: exit 1 if any rewrites would apply (CI mode).

Behaviour:

1. Compile the crate at the *current* edition with a special "rewrite
   mode" flag that emits **all** suggestions (tier5_05), including
   deprecation-lints that would otherwise be warnings.
2. Apply every suggestion tagged `MachineApplicable`.
3. Flag `MaybeIncorrect` suggestions in a report at the end ("the
   following need human review").
4. Update `Riven.toml` edition field.
5. Re-run the compiler on the new edition to verify the crate compiles.

The migrator DOES NOT apply its own transforms — it only executes
suggestions the compiler would already offer. That way:

- One source of truth (the suggestion).
- LSP code-actions and `riven fix` produce identical edits.
- New edition-lint? Write the diagnostic + suggestion once; both tools
  get it.

---

## 5. Architecture / design

### 5.1 Edition representation in the compiler

New module `crates/riven-core/src/edition.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Edition {
    E2026,
    E2027,
    // add as new editions ship
}

impl Edition {
    pub const KNOWN: &'static [(&'static str, Edition)] = &[
        ("2026", Edition::E2026),
        ("2027", Edition::E2027),
    ];

    pub const DEFAULT: Edition = Edition::E2026;
    pub const LATEST: Edition = Edition::E2027;

    pub fn from_str(s: &str) -> Result<Self, String> { ... }

    /// Whether this edition is still supported (not past its sunset).
    pub fn is_supported(self, compiler_version: Semver) -> bool { ... }
}
```

### 5.2 Threading the edition through the pipeline

The edition must be known at lex time (reserved keywords differ per
edition), so it is passed into the entrypoint:

- `rivenc` CLI accepts `--edition=YYYY` (default: read from manifest if
  in a crate; fallback to `Edition::DEFAULT`).
- `riven-cli` build loop reads it from the manifest and passes it to
  `rivenc`.
- `Lexer::new(source, edition)` replaces `Lexer::new(source)`
  (`lexer/mod.rs:1`). A new param.
- `Parser::new(tokens, edition)` similarly.
- `typeck::type_check(program, edition)` similarly.
- Borrow-check doesn't care about edition — it enforces the universal
  rules.
- Codegen doesn't care.

New `EditionCtx` struct (`riven-core/src/edition.rs`):

```rust
pub struct EditionCtx {
    pub edition: Edition,
    pub features: HashSet<String>,  // enabled unstable features
    pub allow_unstable: bool,       // nightly or env var
}
```

Passed as `&EditionCtx` to any pass that needs it.

### 5.3 Per-edition keyword set

`lexer/token.rs:281-311` hard-codes the keyword table. Replace with a
per-edition table:

```rust
pub fn keyword(ident: &str, edition: Edition) -> Option<TokenKind> {
    match ident {
        "let" => Some(TokenKind::Let),
        ...
        // edition-gated:
        "try" if edition >= Edition::E2027 => Some(TokenKind::Try),
        // new reserved word pattern: identifier on 2026, keyword on 2027.
        _ => None,
    }
}
```

On 2026, `try` remains an identifier — code calling `fn try(...)` keeps
working. On 2027, `try` is a keyword — calling `fn try(...)` is an
error, and `riven fix --edition=2027` rewrites it to `fn try_(...)` (or
`r#try` if we adopt Rust-style raw identifiers; see OQ-3).

### 5.4 Per-edition lints / deprecation map

A table in `riven-core` listing all **edition-migration lints**:

```rust
pub struct EditionLint {
    pub name: &'static str,
    pub from: Edition,      // the edition where deprecation fires as warning
    pub to: Edition,        // the edition where it promotes to error
    pub message: &'static str,
    pub rewriter: fn(&Ast, &mut SuggestionBuilder),
}

pub const EDITION_LINTS: &[EditionLint] = &[
    EditionLint {
        name: "hash_to_hashmap",
        from: Edition::E2026,
        to: Edition::E2027,
        message: "type `Hash[K, V]` is renamed to `HashMap[K, V]`",
        rewriter: rewrite_hash_to_hashmap,
    },
    // ...
];
```

On 2026: emit a warning suggesting the rewrite. On 2027: emit an error
with the same suggestion.

### 5.5 Cross-edition linking

This is the hardest part.

**Problem.** A library built on `"2026"` exports `Hash[K, V]`. A
`"2027"` binary wants to call into it. On 2027, `Hash[K, V]` doesn't
exist — it's `HashMap`.

**Solution.** At compile time of the library (2026), the compiler emits
a symbol table metadata file (`*.rivenmeta`) alongside the `*.rlib`.
The metadata records **canonical** names — the names used internally
by the compiler across editions. Consumers see canonical names remapped
through the consumer's edition.

- `Hash[K, V]` on 2026 → canonical `::core::collections::Map[K, V]`.
- `HashMap[K, V]` on 2027 → canonical `::core::collections::Map[K, V]`.
- Identical canonical → linkable.

This implies:

- A **canonical name table** is introduced early (even if there's only
  one edition today) so the 2027 migration doesn't require a retrofit.
- `.rivenmeta` is a new artifact; `crates/rivenc/src/cache/` already has
  a signature extraction step that we extend.
- Incremental compilation (tier-1 project_phase13) fingerprints
  canonical names, not surface names, so cross-edition cache hits work.

### 5.6 Library consumers

A crate on edition 2027 wants to `use foo::Hash` where `foo` is a 2026
crate. Options:

- **Canonicalize on import.** The 2027 compiler sees `foo`'s metadata,
  knows `Hash[K, V]` in 2026 is canonical `Map[K, V]`, and resolves
  `foo::Hash` to the canonical. Downstream, the 2027 crate prefers
  `HashMap` but `use foo::Hash as ThatHash` still works.
- This means **surface names are edition-local; canonical names are
  cross-edition.** Same rule as Rust's paths-to-item resolution.

---

## 6. Implementation plan

### 6.1 New code

- `crates/riven-core/src/edition.rs` — `Edition`, `EditionCtx`,
  `KNOWN_EDITIONS`, helpers.
- `crates/riven-core/src/edition_lints.rs` — `EditionLint` table and
  dispatch logic (invoked from typeck / resolve).
- `crates/riven-core/src/canonical.rs` — canonical name table
  (initially maps every surface type to itself; grows per-edition).
- `crates/riven-cli/src/fix.rs` — `riven fix` subcommand.
- `crates/rivenc/src/fix_mode.rs` — the in-compiler "rewrite mode"
  harness.
- `crates/riven-cli/src/cli.rs:25-113` — add `Fix { edition: Option<String>, dry_run: bool, check: bool }` variant.

### 6.2 Modified code

- `crates/riven-core/src/lexer/mod.rs:70` (`Lexer::new`) — take
  `EditionCtx`.
- `crates/riven-core/src/lexer/token.rs:281` (`keyword`) — per-edition
  dispatch.
- `crates/riven-core/src/parser/mod.rs:41` (`Parser::new`) — take
  `EditionCtx`.
- `crates/riven-core/src/typeck/mod.rs` — pass `&EditionCtx` through.
- `crates/riven-core/src/resolve/mod.rs` — consult
  `EditionCtx.features` when marking an `@[unstable]` use.
- `crates/riven-cli/src/manifest.rs` — validate `edition`, add `[features]`.
- `crates/riven-cli/src/manifest.rs:184` (`validate`) — call
  `Edition::from_str` and return `E4100` on unknown.
- `crates/riven-cli/src/build.rs` — resolve features, pass to `rivenc`.
- `crates/rivenc/src/main.rs:40-68` — `--edition=` flag.
- `crates/rivenc/src/cache/` — include `EditionCtx` fingerprint in the
  cache key so 2026 vs 2027 builds don't cache-hit each other.

### 6.3 Tests

- `crates/riven-core/tests/edition_keyword_gating.rs` — `try` is ident
  on 2026, keyword on 2027.
- `crates/riven-core/tests/edition_rewrites.rs` — `Hash[…]` → `HashMap[…]`
  suggestion fires on 2026, error on 2027.
- `crates/riven-cli/tests/fix_migrator.rs` — run the migrator on a small
  fixture, assert the rewritten source compiles on the new edition.
- `crates/riven-cli/tests/cross_edition_linking.rs` — fixture with two
  crates: lib (2026), binary (2027); assert it builds and runs.

### 6.4 Documentation

- `docs/reference/07-editions/00-edition-policy.md` — the policy in §4.
- `docs/reference/07-editions/01-edition-2026.md` — current edition.
- `docs/reference/07-editions/02-edition-2027.md` — delta document.
- CHANGELOG — every PR that adds an edition lint must have an entry.

---

## 7. Interactions with other tiers

- **Tier 5 doc 03 (attributes):** `@[stable]`, `@[unstable]`,
  `@[deprecated]`, `@[rustc_since]`-like — shared syntax. Editions use
  them extensively.
- **Tier 5 doc 05 (suggestions):** the migrator `riven fix` is the
  canonical consumer of machine-applicable suggestions. Any feature that
  goes through an edition deprecation must ship with a suggestion.
- **Tier 5 doc 04 (error codes):** `E4100-E4199` reserved for
  edition/manifest errors (§5.1 of doc 04 also reserves this range).
- **Tier 5 doc 01 (reference):** edition-scoped chapters.
- **Tier 1 (stdlib):** stdlib-wide renames (e.g. `Hash[K, V]` →
  `HashMap[K, V]`) are edition lints. Tier-1 B3 is the first such
  candidate.
- **Tier 1 concurrency / async:** reserved keywords (`async`, `await`,
  `actor`, `spawn`, `send`, `receive`) become real keywords on a future
  edition. Today they are reserved (tier1 B6).
- **Project_phase13_incremental:** cache keys MUST include
  `EditionCtx::fingerprint()`.

---

## 8. Phasing

### Phase 2a: plumbing (1-2 weeks)

1. `Edition` enum + `EditionCtx` + default values.
2. Thread through `Lexer::new` and `Parser::new`. Default to
   `Edition::DEFAULT` from every call site.
3. Read `package.edition` in `riven-cli/src/build.rs`; pass
   `--edition=...` to `rivenc`.
4. `rivenc --edition=...` flag.
5. Validation in `Manifest::validate` + `E4100` error code.

**Exit criterion:** `edition = "2026"` round-trips through the pipeline
and is observable in `--emit=ast` output. Nothing changes semantically.

### Phase 2b: first edition-lint (1-2 weeks)

Use B3 (Hash→HashMap) as the canary:

1. Land `HashMap[K, V]` as an alias of `Hash[K, V]` — tier-1 B3.
2. Register an `EditionLint { from: E2026, to: E2027, ... }` that warns
   on 2026 and errors on 2027.
3. Add `Edition::E2027` to `KNOWN`. It's not the default.
4. `riven fix --edition=2027` test fixture with one file.

**Exit criterion:** a two-crate fixture compiles on 2026, fails on 2027
with E???? + suggestion, migrates cleanly.

### Phase 2c: features (2-3 weeks)

1. `[features]` section in `Riven.toml`.
2. `@[unstable(feature = "...")]` attribute recognition (requires
   tier5_03 phase 3a).
3. `features = ["..."]` resolution and checking.
4. `E4103: use of unstable feature '...' requires manifest feature
   opt-in`.

### Phase 2d: canonical names + cross-edition (3-4 weeks)

1. Introduce `canonical.rs` — initially every surface → same canonical.
2. Emit `.rivenmeta` with canonical-name table.
3. On import, resolve through canonical.
4. Fixture: 2026 lib ↔ 2027 bin.

### Phase 2e: deprecation window enforcement (small)

1. `E4102` when an edition passes its sunset date.
2. `Edition::is_supported` logic.
3. CHANGELOG entry pattern.

**Total:** ~7-11 weeks for a full edition infrastructure. Phases 2a and
2b can ship in a patch release; 2c-2e over the subsequent minor.

---

## 9. Open questions & risks

### OQ-1. Is the editions model even right for Riven?

The Go team argues editions are an anti-pattern: they split the
ecosystem and create compiler complexity. Rust's experience: edition
machinery has paid for itself many times (2018 `?` operator, 2021
disjoint captures, 2024 `Cell::new`).

**Recommended stance (confirms overview §7.5):** Riven adopts editions.
Justification:

- Riven is pre-1.0. Choosing no-editions means *every* cleanup we want
  in the next 5 years becomes impossible or a major-version break.
- Riven inherits Rust's ownership model and will discover the same
  "oh, if we had designed this differently…" cases. Rust needed
  editions to fix closure-capture inference in 2021; we will too.
- We mitigate Go's objection with **strict constraints** (§4.2): no
  semantics-changing editions; canonical names; cross-edition linking.
- **Accept the cost:** multi-edition compiler complexity and
  migration-tool maintenance are the price of being able to evolve.

### OQ-2. Edition default: latest or oldest-supported?

**Recommended:** the compiler's *default* edition is the oldest
still-supported one. That way, running `rivenc foo.rvn` (no manifest,
no flag) is maximally conservative — it'll accept old code.

Rust defaults to `2015` (oldest) for the same reason. We follow.

### OQ-3. Raw identifiers for ex-keywords?

If `try` becomes a keyword on 2027, a 2026 library might export a
function literally named `try`. On 2027, callers cannot spell the name.

**Recommended:** introduce Rust-style `r#try` as a raw-identifier syntax
in edition 2027's spec. The migrator rewrites `foo.try(...)` callsites
to `foo.r#try(...)` automatically.

Alternative: forbid an edition from reserving a name that existed as a
public identifier in a supported previous edition. Much more restrictive
and not what Rust chose.

### OQ-4. What about patch-level compat within an edition?

**Recommended:** within a single edition line, each minor release MUST
accept all programs the previous minor release accepted. Informally:

- 0.2.x on `edition = "2026"` accepts program P.
- 0.3.x on `edition = "2026"` must also accept P.
- 0.3.x on `edition = "2027"` may reject P.

This is documented in `docs/reference/07-editions/00-edition-policy.md`
and enforced by regression fixtures.

### OQ-5. Should `edition` be required (not optional)?

Today it's `Option<String>`. **Recommended:** make it optional at parse
time (to tolerate old manifests) but **warn** on omission ("no edition
specified in Riven.toml; defaulting to 2026; add an explicit
`edition = \"2026\"`"). In a far-future edition, promote to error. Rust
did the same for 2015.

### OQ-6. Stability channels (stable / beta / nightly)?

Rust has three; Go has one; Swift has one. **Recommended:** start with
one channel (stable), with an `RIVENC_ALLOW_UNSTABLE=1` env var for
devs who need to play with unstable features. When unstable-feature
volume grows (phase 2c+), add a formal nightly build target. Deferring
this keeps the initial work small.

### OQ-7. Risk: canonical name table is a single-point-of-truth that
      gets out of sync.

Mitigation:

- Test harness: every type/trait/fn that has surface-to-canonical
  mapping gets a fixture asserting round-trip through the metadata.
- Library authors cannot directly write canonical names (they're
  compiler-internal). So the risk is confined to compiler contributors.

### OQ-8. Risk: the migrator can't handle complex cases and users hit
      `MaybeIncorrect` walls.

This is real. Rust's `cargo fix --edition` works but occasionally needs
manual help. Our `MaybeIncorrect` suggestions (tier5_05) let the user
review before applying. Acceptance: the migrator is not promised to be
flawless, but it must be *safe* (never break compiling code silently).
Emitting a suggestion with `MachineApplicable` that then doesn't
compile after application is a P0 bug.

### OQ-9. Risk: cross-edition linking fails subtly.

Mitigation: the test suite in `tests/cross_edition_linking.rs` is a
regression net. Every new edition PR must add a fixture exercising
import-from-previous-edition.

---

## 10. Acceptance criteria

- [ ] `Edition` enum with at least `E2026` + `E2027` variants.
- [ ] `Manifest::validate` rejects unknown editions with `E4100`.
- [ ] `Manifest::validate` warns on missing edition with an informational
      diagnostic.
- [ ] `lexer` / `parser` / `resolve` / `typeck` all take `&EditionCtx`.
- [ ] Cache key includes `EditionCtx::fingerprint()`.
- [ ] `[features]` table is parsed from `Riven.toml`.
- [ ] `@[unstable(feature = "x")]` + `features = ["x"]` opt-in works;
      without opt-in, error `E4103`.
- [ ] `RIVENC_ALLOW_UNSTABLE=1` (or nightly channel) bypasses.
- [ ] At least one `EditionLint` exists and is exercised: warning on
      2026, error on 2027.
- [ ] `riven fix --edition=2027` migrates a fixture crate: rewrites
      source, updates manifest, the result compiles.
- [ ] Cross-edition linking fixture: 2026 lib + 2027 bin builds and
      runs.
- [ ] `E4102` fires when compiling against an unsupported edition.
- [ ] `docs/reference/07-editions/` exists with policy + per-edition
      docs.
