# Tier 5 — Language Reference (Formal Grammar + Normative Prose)

Status: draft
Depends on: tier5_03 (attributes — shared syntax), tier5_02 (editions — the
reference must be per-edition), tier5_04 (error codes — referenced from
normative-error sections).
Blocks: external implementers, stability claims ("Riven supports X"),
future language lawyering.

---

## 1. Summary & motivation

Riven has no reference. There is a tutorial (`docs/tutorial/01-16`) aimed
at readers, and an implementation (`crates/riven-core`) that is the
**only** authoritative description of what the language accepts. That is a
failure mode:

- External implementers cannot know what is deliberate vs accidental. The
  Pratt-parser precedence table at `parser/expr.rs:10-51` is the truth;
  nothing in the tutorial echoes it. If we rewrite the parser, nothing
  short of a diff tells us that precedence changed.
- Compiler contributors cannot know which behaviours are contractual.
  `typeck/coerce.rs:49-82` encodes specific coercion rules (`&mut T → &T`,
  `&String → &str`, int widening, Option-covariance, class subtyping). Any
  of these could be "stable" or "I tried this and it worked"; there is no
  way to tell today.
- Users hit subtle behaviours (e.g. newline-as-terminator in
  `lexer/token.rs:227-266`) with no document to consult, so they file
  "bug" reports against intentional behaviour.
- The language cannot grow. Anyone proposing a change can't say what
  they're changing relative to.

This document specifies the structure, production pipeline, and
sync-with-the-compiler discipline for a Riven Language Reference.

---

## 2. Current state

### 2.1 What exists (informal)

- **Tutorial.** `docs/tutorial/01-getting-started.md` through
  `docs/tutorial/16-formatting-and-comments.md`. Sixteen chapters of
  user-oriented prose. No grammar, no precedence table, no normative
  sections. Examples but no counter-examples of rejected programs.
- **Inline doc comments** on many functions, particularly in
  `crates/riven-core/src/parser/` and `hir/types.rs`.
- **`docs/requirements/tier1_*.md`** (which you're reading the tier-5
  sibling of) — design intent, not reference material.

Directories that do **not** exist:
- `docs/reference/` — no files.
- `docs/errors/` — no files (needed for doc 04 of this tier).
- `SPEC.md`, `GRAMMAR.md`, `REFERENCE.md` anywhere (grep across the repo
  returns zero hits).

### 2.2 Implicit grammar locations

The following files are the **de facto** grammar today. They need to be
extracted into normative form:

- **Lexer.** `crates/riven-core/src/lexer/mod.rs` (842 lines) and
  `crates/riven-core/src/lexer/token.rs` (414 lines). Token set at
  `token.rs:56-222`. Line-continuation rules at `token.rs:227-266`.
  Keyword table at `token.rs:281-311`.
- **Parser.** `crates/riven-core/src/parser/mod.rs` (1791 lines, top-level
  items), `parser/expr.rs` (1544 lines, expressions + precedence),
  `parser/types.rs` (473 lines, type-expression grammar),
  `parser/patterns.rs` (479 lines, patterns).
- **Precedence.** `parser/expr.rs:10-51` — 12 levels, binding-power pairs.
- **Postfix operators.** `parser/expr.rs:98-104` — `.`, `?.`, `[...]`,
  `?`, `(...)`.
- **Attribute syntax.** `parser/mod.rs:1572-1610` (`parse_attributes`) and
  `parser/mod.rs:473-511` (dispatch).
- **Resolution rules.** `crates/riven-core/src/resolve/mod.rs` (2861
  lines) — scope, shadowing, visibility.
- **Type rules.** `crates/riven-core/src/typeck/` (coerce 172 lines, infer
  1015 lines, unify 269 lines, traits 285 lines).
- **Ownership / borrow rules.** `crates/riven-core/src/borrow_check/` —
  regions (`regions.rs`), NLL (`borrows.rs`), elision
  (`lifetimes.rs:55-85`).

### 2.3 Conventions already visible

Even without a reference, some conventions surfaced in the code:

- `.rvn` file extension.
- Line comments: `#` (`lexer/mod.rs:70`); block comments nested with
  `#= ... =#` (tutorial ch. 16).
- String interpolation: `"hello #{expr}"`
  (`token.rs:206-207` — `InterpolatedString(Vec<StringPart>)`).
- Doc comments: `##` (tutorial ch. 16).
- Case conventions: `snake_case` for fns/locals, `UpperCamelCase` for
  types, `SCREAMING_SNAKE_CASE` for constants, `'a` for lifetimes
  (documented in tutorial ch. 16, enforced by the formatter where
  applicable).

---

## 3. Goals & non-goals

### 3.1 Goals

- A **normative grammar** (EBNF-with-extensions) covering lex + parse +
  type-expression grammar + pattern grammar. One section per construct.
- **Normative prose chapters** for semantic rules: name resolution,
  ownership, borrow check, lifetime elision, coercion rules, trait
  resolution order, const evaluation, method resolution.
- **Examples and counter-examples** for every rule (machine-checked, see
  §5.3).
- Separation of **normative** (required for conformance) from
  **informative** (explanatory; may change).
- An **edition-scoped** reference (tier5_02): each edition gets a delta
  document.
- **Stability guarantees** for the reference itself — "Normative" sections
  change only between editions; "Informative" sections may change with any
  minor release.

### 3.2 Non-goals

- Executable specification / reference implementation in a separate
  language. The reference is documentation, not code.
- Machine-translatable ISO-style spec (numbered paragraphs à la C++). Too
  much overhead for a single-implementation language. Plain Markdown with
  anchor IDs is enough.
- ABI stability at the C level (that's tier1_00 §7).
- Turing-complete macro semantics (tier1_05 is the macro doc; the
  reference cites it, doesn't re-specify it).

---

## 4. Surface — table of contents

The proposed TOC. Each chapter maps to a directory/file under
`docs/reference/`. Anchor IDs are stable once published.

### 4.1 Structure

```
docs/reference/
├── README.md                         # entry point; edition badge
├── 01-conventions.md                 # grammar notation, "normative", etc.
├── 02-lexical/
│   ├── 00-source.md                  # file encoding, BOM, line endings
│   ├── 01-whitespace-and-comments.md
│   ├── 02-keywords.md                # full table incl. reserved words
│   ├── 03-identifiers.md
│   ├── 04-literals.md                # int, float, string, char, interp
│   ├── 05-operators-and-punctuation.md
│   └── 06-newline-handling.md        # the line-continuation rules
├── 03-grammar/
│   ├── 00-overview.md                # full EBNF index
│   ├── 01-items.md                   # top-level items: def, class, ...
│   ├── 02-statements.md
│   ├── 03-expressions.md             # + precedence table
│   ├── 04-patterns.md
│   ├── 05-types.md                   # type-expression grammar
│   ├── 06-attributes.md              # @[...] (cites tier5_03)
│   └── 07-generics.md
├── 04-names/
│   ├── 00-resolution.md
│   ├── 01-paths-and-use.md
│   ├── 02-visibility.md              # pub / pub(crate) / private
│   └── 03-shadowing.md
├── 05-types/
│   ├── 00-primitive-types.md
│   ├── 01-references-and-lifetimes.md
│   ├── 02-collection-types.md
│   ├── 03-user-defined-types.md      # struct / class / enum / newtype
│   ├── 04-trait-objects-and-dyn.md
│   └── 05-inference.md               # normative inference algorithm
├── 06-semantics/
│   ├── 00-evaluation-order.md
│   ├── 01-coercions.md               # normative; cites coerce.rs rules
│   ├── 02-ownership-and-move.md
│   ├── 03-borrowing.md
│   ├── 04-lifetime-elision.md
│   ├── 05-trait-resolution.md        # nominal vs structural priority
│   ├── 06-method-resolution.md       # includes auto-deref
│   ├── 07-drop-order.md              # (cites tier1_04)
│   └── 08-unsafe.md
├── 07-editions/
│   ├── 00-edition-policy.md          # (cites tier5_02)
│   ├── 01-edition-2026.md            # the "current" edition's list
│   └── 02-edition-2027.md            # future edition deltas
├── 08-errors/
│   └── index.md                      # links into docs/errors/ (tier5_04)
└── 09-glossary.md
```

### 4.2 Grammar notation

The reference uses an extended EBNF:

```
# Comments start with '#' in grammar blocks.
rule-name   ::= alternative-1
              | alternative-2
              ;

grouping    ::= ( X Y )       # parentheses group
optional    ::= X ?            # zero or one
star        ::= X *            # zero or more
plus        ::= X +            # one or more
delim-list  ::= X (',' X)*     # comma-separated list (idiom)

'literal'   ::= the literal text 'literal'
<TOK>       ::= the lexer token <TOK>, defined in 02-lexical
```

Pratt-style precedence is expressed by a **side table** (see 03-expressions)
because true EBNF for infix-with-precedence is unreadable. Example:

| Level | Operator(s)         | Assoc   | Note                              |
|-------|---------------------|---------|-----------------------------------|
| 1/2   | `=` `+=` `-=` etc.  | right   | assignment                        |
| 3/4   | `\|\|`              | left    | logical or                        |
| 5/6   | `&&`                | left    | logical and                       |
| 7/8   | `==` `!=` `<` ... | non-assoc | comparison (chaining = parse err) |
| …     | …                   | …       | (mirrors `parser/expr.rs:10-51`)  |

### 4.3 Normative vs informative

Every chapter has frontmatter:

```
---
status: normative   # or: informative
edition: 2026       # first edition in which this is normative
since: 0.2.0        # compiler version that first implemented this rule
---
```

- **Normative** text: implementations claiming conformance MUST obey it.
- **Informative** text: explanatory; not binding.

Conventions:

- Grammar tables: **normative**.
- Examples: **informative** unless introduced with "**Example (normative):**".
- "Implementation note:" callouts: **informative** — describe a current
  `riven-core` design choice, not a mandated behaviour.
- Error codes in prose: referenced by `E????` and linked into
  `docs/errors/`.

---

## 5. Architecture / design

### 5.1 Source format

- Plain Markdown. Renders on GitHub without a build step.
- Conforms to the CommonMark spec + GFM tables.
- Grammar blocks: fenced with ` ```ebnf ` so a future renderer can
  syntax-highlight / validate.
- Anchor IDs: `{#rule-id}` on every production. Stable across patch
  versions of the reference.

### 5.2 Chapter ordering principle

Bottom-up: lexical → grammar → names → types → semantics. Each chapter
may forward-reference ("see 06-semantics") but only backward-depends.

### 5.3 Keeping the reference in sync with the compiler — the fixture
    harness

**This is the most important design decision in the doc.** Three options
surveyed; Option B is recommended.

**Option A — generate the reference from the parser grammar.**
Tools like Ohm, LALRPOP, or PEG generators can emit a grammar doc. We
don't have such a generator — the parser is hand-written
(`parser/expr.rs:1` comment calls it "Pratt-style precedence climbing").
Introducing one would be a rewrite. Additionally, semantic rules (coercions,
trait resolution order) cannot be generated from code — they'd be hand-
written anyway.
**Rejected** — requires a parser rewrite for marginal gain.

**Option B — test-fixture backing, prose written by humans. [RECOMMENDED]**
Every grammar production has at least one `.rvn` fixture under
`crates/riven-core/tests/reference/<chapter>/`. A dedicated integration
test `tests/reference_coverage.rs` does two things:

1. Runs each fixture through the pipeline and asserts the expected
   outcome (accept / reject with specific `ErrorCode`).
2. Cross-checks: for every `{#anchor-id}` in `docs/reference/`, at least
   one fixture has a `# reference: anchor-id` header comment. Missing
   anchors fail CI.

Naming: `tests/reference/03-expressions/02-precedence-mul-over-add.rvn`.
Each fixture has a header:

```
# reference: expressions-precedence-mul-over-add
# expect: accept
# expect-hir: (BinOp (Add (Int 1) (BinOp (Mul (Int 2) (Int 3)))))
```

And for rejection fixtures:

```
# reference: ownership-use-after-move
# expect: reject
# expect-error: E1001
# expect-span: 3:7
```

**Advantages:**
- Prose is human-readable.
- Tests catch silent regressions — if precedence changes, the expected
  HIR no longer matches.
- Anchor coverage forces the author of a new rule to write prose.

**Disadvantages:**
- Fixtures drift from prose unless reviewed together — PR template should
  require "reference updated? fixtures updated?" checkboxes.
- Verbose: tight correspondence adds ~1 fixture per rule, i.e. hundreds.

**Option C — pure prose discipline, no enforcement.**
What most languages do before they become popular. Works only with a
tiny, hyper-disciplined team.
**Rejected** for Riven because by the time divergence bites, it'll be
too late to fix without a "spec-vs-compiler" rewrite.

### 5.4 Edition scoping

Each chapter has a `edition: 2026` or `edition: 2027` frontmatter field. A
future edition may replace a chapter with a new version; old editions stay
reachable at `docs/reference/07-editions/01-edition-2026.md`-linked URLs.

For chapters that change between editions, the delta is documented in
`07-editions/NN-edition-YYYY.md` as a bullet list ("new keyword `foo`",
"syntax `Hash[K,V]` removed, use `HashMap[K,V]`").

### 5.5 Worked examples embedded as fixtures

Every normative section SHOULD contain at least one example. The example
is a **fixture** — so it's syntax-checked. Use inline include:

```markdown
Addition binds tighter than subtraction.

\```rvn
{{#fixture: 03-expressions/02-precedence-mul-over-add.rvn}}
\```
```

A preprocessor (tiny Rust tool `tools/refgen/`) expands the include at
render time. For GitHub-native rendering (no build), we ship the expanded
version committed; CI verifies expansion is up to date.

---

## 6. Implementation plan — files to touch / create

### 6.1 New files

- `docs/reference/README.md`, full chapter tree (~35 files — one per §4.1
  entry).
- `crates/riven-core/tests/reference/` — fixture tree mirroring chapters.
- `crates/riven-core/tests/reference_coverage.rs` — anchor-to-fixture
  cross-check.
- `tools/refgen/` (tiny Rust binary) — fixture include expansion, anchor
  lint.
- `docs/reference/.templates/chapter.md` — frontmatter template.

### 6.2 Code changes

- `crates/riven-core/src/lib.rs` — expose a `reference_anchors()` helper
  that returns the full list of anchor IDs referenced by the compiler's
  error-reporting code (so `refgen` can fail-fast on typos). Optional; not
  blocking.
- No changes to the parser/lexer/typeck required for v1 of the reference.

### 6.3 CI wiring

- `cargo test --test reference_coverage` added to the workspace default
  test target.
- A `markdown-lint` or `lychee` (link checker) step.
- A `spellcheck` step over `docs/reference/` using codespell (cheap, high
  signal).

### 6.4 Initial content seeding (priority order)

The reference can ship incrementally. Priority for v0.3:

1. **01-conventions.md** + **02-lexical/** — straightforward to transcribe
   from `lexer/token.rs`. ~2 days.
2. **03-grammar/03-expressions.md** including the precedence table. Draw
   from `parser/expr.rs:10-51`. ~1 day.
3. **03-grammar/05-types.md** — type expressions. Draw from
   `parser/types.rs`. ~1 day.
4. **06-semantics/01-coercions.md** — normative list from
   `typeck/coerce.rs:29-113`. ~1 day.
5. **06-semantics/04-lifetime-elision.md** — cite the three Rust elision
   rules as implemented in `borrow_check/lifetimes.rs:55-85`. ~1 day.
6. **06-semantics/02-ownership-and-move.md** + **03-borrowing.md**. Draw
   from `borrow_check/`. ~3 days.

Total initial seed: ~10 days to publishable draft.

---

## 7. Interactions with other tiers

- **Tier 5 doc 02 (editions):** the reference is edition-scoped. Its
  chapters carry edition frontmatter; new editions add delta documents in
  `07-editions/`.
- **Tier 5 doc 03 (attributes):** `03-grammar/06-attributes.md` normatively
  specifies the `@[name(args)]` form that deprecation/stability attrs use.
- **Tier 5 doc 04 (error codes):** `08-errors/index.md` links to
  `docs/errors/E????.md`. Every error code the reference mentions must
  exist in the registry.
- **Tier 5 doc 05 (suggestions):** the reference may cite machine-applicable
  suggestions as examples in the relevant semantics chapter.
- **Tier 1 (stdlib):** the reference covers the **language**, not the
  library. Stdlib surface lives in autogenerated API docs
  (`cargo doc`-equivalent — future work). A pointer chapter links the two.
- **Tier 3 (LSP):** the LSP's hover info may include a "see reference:
  §anchor" link once stable URLs exist. Not blocking.

---

## 8. Phasing

### Phase 1a: scaffolding (1 week)

1. Create `docs/reference/` tree with empty chapters + frontmatter.
2. Commit `tools/refgen/` skeleton (CI-visible, non-blocking lint).
3. Commit `tests/reference_coverage.rs` with an allow-empty mode that
   lists missing anchors as warnings, not failures.
4. Seed ch. 01 (conventions).

### Phase 1b: lexical + grammar (2 weeks)

1. Chapter 02 (lexical), all subchapters.
2. Chapter 03 (grammar): items, statements, expressions, patterns, types,
   attributes, generics.
3. Fixtures for every production.

### Phase 1c: names + types (2 weeks)

1. Chapter 04 (names): resolution, visibility, shadowing, paths.
2. Chapter 05 (types): primitives, references, user-defined, traits,
   inference.

### Phase 1d: semantics (3 weeks)

1. Chapter 06 (semantics): coercions, ownership, borrowing, lifetime
   elision, trait resolution, method resolution, drop order, unsafe.

### Phase 1e: finalisation (1 week)

1. Flip `tests/reference_coverage.rs` from warn to fail.
2. Publish.
3. Add PR template requiring "reference updated?".

**Total:** ~9 weeks of writing, can parallelise with other tier-5 work.

---

## 9. Open questions & risks

### OQ-1. Grammar formalism
**Recommended:** EBNF-with-extensions + precedence side tables (§4.2).
Pure PEG would match the Pratt parser but hides ambiguity. Pure BNF is
unwieldy for lists. ANTLR-style is executable but we don't want a
parser-generator dependency.

### OQ-2. Which grammar to use for tokens?
**Recommended:** regex-style for lexer productions, EBNF for parser
productions. E.g. `<INT> ::= [0-9]+ ('_' [0-9]+)*`. Consistent with most
language references.

### OQ-3. Where do Pratt-precedence rules formally live?
**Recommended:** a single normative **table** in
`03-grammar/03-expressions.md` reproducing the binding-power pairs from
`parser/expr.rs:10-51`. EBNF-with-precedence-comments is too easy to
misread.

### OQ-4. Line-sensitivity / significant newlines — how do we express
    this in grammar?
`parser/expr.rs:73` calls `skip_newlines_if_continuation()`, and the
token set at `lexer/token.rs:227-266` lists every operator after which a
newline is continuation rather than terminator. **Recommended:** a
**parallel prose section** ("Newline handling", `02-lexical/06-…`) lists
the continuation tokens and describes the algorithm. Grammar productions
that span newlines carry a `<continuation>` meta-terminal.

### OQ-5. How to version the reference independently of the compiler?
The reference has its own version like `1.0.0-edition-2026`. It is
bumped at compiler releases and has its own changelog. The `edition`
binds it to a specific major compiler line.

### OQ-6. Risk: the reference rots if contributors skip updating it.
Enforcement: (a) PR template, (b) `reference_coverage` test, (c) CI lint
that new `ErrorCode` variants without `docs/errors/E????.md` fail the
build.

### OQ-7. Risk: natural-language ambiguity in normative prose.
Mitigation: (a) fixtures are the final arbiter — the compiler's behaviour
on the fixture is what counts; prose is best-effort explanation; (b) a
"Conformance disputes" section in 01-conventions.md directs implementers
to file an issue with a reduced fixture.

### OQ-8. What about the standard library "appendix"?
The reference covers only the language. Stdlib goes in a separate
`docs/stdlib/` (future work), which is auto-generated from doc comments.
The reference's ch. 08 has a pointer stub.

### OQ-9. Risk: someone writes a competing compiler and diverges.
A language reference is what makes compatible implementations *possible*.
Absent one, divergence is unavoidable. Publishing the spec is the point.
We accept this risk.

---

## 10. Acceptance criteria

- [ ] `docs/reference/` exists with the full chapter tree from §4.1.
- [ ] Every chapter has frontmatter declaring `status:` and `edition:`.
- [ ] `tests/reference_coverage.rs` passes with **zero** missing-anchor
      warnings.
- [ ] At least one fixture exists per normative production.
- [ ] The precedence table in `03-grammar/03-expressions.md` byte-for-byte
      matches (a comment cross-references) `parser/expr.rs:10-51`.
- [ ] The coercion table in `06-semantics/01-coercions.md` covers every
      arm of `typeck/coerce.rs:29-113`.
- [ ] The elision rules section names all three rules as implemented in
      `borrow_check/lifetimes.rs:57-85`.
- [ ] A new compiler release requires a reference changelog entry (PR
      template item).
- [ ] External implementer (real or dogfood) writes a minimal parser
      using only `docs/reference/` and successfully parses at least one
      sample fixture.
