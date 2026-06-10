# ADR: Syntax-parity harness — one syntax across every delivered surface

Status: Accepted (2026-06-10, `feat/drop-elaboration`)

## Context

Ruxen ships five surfaces that each consume Ruxen source: the **compiler**
(`ruxen compile/build/check/test`), the **formatter** (`ruxen fmt`), the
**REPL** (`ruxen repl`), the **LSP** (`ruxen lsp`), and the **IDE** analysis
layer (`src/ruxen_ide`, which the LSP and any editor integration drive). They
share the same `lexer` + `parser`, but they call DIFFERENT entry points and
re-emit through different code:

- the formatter re-emits source from the AST (`formatter/format_items.rs`,
  `format_expr.rs`) — a missing item-kind arm or a precedence-blind re-emit
  silently *drops or rewrites* syntax;
- the REPL dispatches by leading token (`parser/mod.rs::parse_repl_input`) — a
  contextual-keyword item (e.g. `alias`, which lexes as an `Identifier`) needs
  an explicit route or it falls into the expression arm and is rejected;
- binary crates pattern-match exhaustively on `TopLevelItem`/`MixinItem`/
  `ImplItem` — a new AST variant can miss an arm.

USER REQUIREMENT: *"none of the lsp/ide/fmt/compiler/repl may diverge on any
syntax of ruxen; ruxen syntax must be 100% available on every package we
deliver"* + *"we need tests for the syntax parity on all binaries and
libraries."*

This is a recurring real-bug source. The alias feature needed a REPL dispatch
route and a formatter arm added or items were silently dropped. Building this
harness immediately surfaced **five** live `ruxen fmt` destructiveness bugs
(see "Divergences found").

## Decision

A two-axis, committed harness makes one-syntax-everywhere a tested invariant.

### Axis 1 — per-surface conformance over a defined corpus

**Corpus** (one place; auto-discovered, no per-file registration):

| Source | Depth |
|---|---|
| `tests/release-e2e/cases/*.rx` | full programs |
| `library/std/**/*.rx` | fragments (don't compile alone) |
| `examples/**/*.rx` | full programs |
| `../canvas/src/**.rx`, `../quiver/src/**.rx`, `../rondo/src/**.rx` | sibling libraries, **read-only** |

A stdlib/sibling fragment cannot type-check standalone, so the **per-corpus
depth** is calibrated per surface:

- **compiler**: lex + **parse** every file. (A full compile is owned by the
  existing release-e2e `compile` phase for the self-contained `cases/`; parse
  is the right floor for fragments and is the exact thing every surface
  shares.)
- **fmt**: `format(parse(src))` must re-parse to a **structurally identical
  AST** (spans ignored; see "Structural oracle") **and** be **idempotent**
  (`fmt(fmt(x)) == fmt(x)`). This is the strongest anti-divergence check — it
  catches Q23/Q30/Q34-class destructiveness forever.
- **repl**: `parse_repl_input` must **accept** every top-level item kind the
  batch parser accepts — exercised both by a per-kind exemplar set and by a
  sweep that feeds every corpus file's per-item source slice to the REPL.
- **lsp + ide**: both funnel through `ruxen_ide::analysis::analyze`, which
  **gates on parse** (`program: None` on a lex/parse error). The contract is
  exact: *if the shared parser accepts a file, `analyze().program` must be
  `Some`* — otherwise the IDE/LSP would raise a syntax error the compiler
  never does.

### Axis 2 — structural pins

**(a) Exhaustiveness guard.** `tests/syntax_parity.rs` contains
`guard_top_level_item` / `guard_mixin_item` / `guard_impl_item`: never-called
functions that `match` each item enum with **no `_ =>` arm**. Adding a variant
to `TopLevelItem`/`MixinItem`/`ImplItem` makes the match non-exhaustive → the
test crate fails to COMPILE → the author MUST make a conscious parity decision
(extend the harness, or add an arm with a ledger note). Compile-time arm
coverage is the cheapest honest guard. (`ExprKind` already has this guarantee
via `parser/visit.rs`'s exhaustive `walk_*`, which the formatter's
`collect_node_spans` rides on.)

**(b) Intentional-divergence allowlist.** Constructs the *shared parser*
accepts (so fmt/lsp/ide round-trip them) but the *direct-compile path* rejects
with a specific diagnostic are an EXPLICIT table (`INTENTIONAL_DIVERGENCES`),
not an accident:

| Construct | Code | Why accepted-but-rejected |
|---|---|---|
| top-level bare expression (`Tester.describe(...) do … end`) | E0728 | parser accepts so `ruxen fmt` round-trips test files; direct compile rejects (`ruxen test` hoists into a synthesised `def main` first). |
| `@[repr(...)]` / `@[...]` prefix attribute | E0607 | retired surface; the parser rejects at parse time (documented in `parser/CLAUDE.md`), so it never reaches the corpus — listed here for the record, enforced by the parser. |

The test asserts the **acceptance** half (each still parses); the rejection
half is owned by the resolve-phase tests that emit the code.

### Structural oracle (span-blind, import-order-tolerant)

Two ASTs are "structurally identical" iff their **AST-printer fingerprint**
matches. The printer (`parser/printer`) is span-free and **fully
parenthesises every operator**, so distinct trees print distinctly — making it
a sound reparse-identity oracle without hand-zeroing spans across every node
kind. One normalisation: `Use std.x.{a, b}` member lists are **sorted** before
comparison, because `ruxen fmt` deliberately alphabetises imports
(`format_imports.rs`) — a meaning-preserving canonicalisation. A *dropped*,
*added*, or *renamed* member still changes the fingerprint and trips the test.

## Delivery

- `compiler/ruxen_core/tests/syntax_parity.rs` — axes compiler / fmt / repl +
  the exhaustiveness guard + the intentional-divergence allowlist (in-process).
- `src/ruxen_ide/tests/syntax_parity_ide.rs` — axes lsp / ide (needs the
  `ruxen_ide` dep).
- `tests/release-e2e/run.sh` — a new **`parity`** phase (in `PHASES=all`)
  driving the SHIPPED `ruxen fmt` binary over every package for parse +
  binary-level idempotence.
- Pin: `compiler/ruxen_core/tests/q34_fmt_grouping_parens.rs` (the Q34 fix).

## Divergences found while building this (all FIXED — acceptance gaps, not
intentional rejections)

1. **Q34** — `ruxen fmt` dropped grouping parens
   (`(a + b) / c` → `a + b / c`), silently changing arithmetic. Fixed by
   re-parenthesising operands by precedence (`formatter/prec.rs`, single
   source mirrored from `parser::expr::infix_binding_power`).
2. **Zero-arg `MethodCall`** → bare field access (`s.bytes()` → `s.bytes`). A
   `MethodCall` and a `FieldAccess` are distinct AST nodes; always emit `()`.
3. **Method visibility section dropped** (`private` method round-tripped as
   public). Class/struct/enum bodies now emit `private`/`protected`/`public`
   **section markers** as the running visibility changes.
4. **`async` modifier dropped** (`async def f` → `def f`). Now emitted.
5. **(catalogued, not a bug)** import-member reordering — intentional fmt
   canonicalisation; handled by the oracle's `use`-set normalisation.

## How a future feature author extends the corpus (ONE place)

1. Add a `.rx` file under `tests/release-e2e/cases/` (full program) or rely on
   the auto-swept stdlib/sibling sources — no registration needed.
2. If the feature adds a new `TopLevelItem`/`MixinItem`/`ImplItem` variant, the
   exhaustiveness guard fails to compile until you add an arm (and the
   formatter/repl handling it points to).
3. If the feature is a contextual-keyword item kind, add a one-line exemplar to
   `repl_exemplars()` so the REPL-dispatch route is pinned.
4. If the feature is an accepted-but-compile-rejected surface, add a row to
   `INTENTIONAL_DIVERGENCES` with the code + rationale.
