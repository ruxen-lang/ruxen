# 23 — Phase 5: language reference (T5.01)

**Depends on:** prompts 01-15 (every language feature must exist
before it can be referenced).
**Reads:** `docs/requirements/tier5_01_language_reference.md`.

## Goal

`docs/reference/` becomes the canonical Riven language spec. Per-
chapter coverage of every surface feature.

## Chapters (mandatory)

1. Introduction + design philosophy
2. Lexical structure (tokens, comments, doc comments)
3. Identifiers + keywords (reserved list)
4. Items (def, struct, class, enum, mixin, include, extension, use)
5. Types (primitives, references, tuples, arrays, generics, const
   generics, `some Mixin`, `any Mixin`, opaque types)
6. Expressions (operators, control flow, match, closures, async,
   .await)
7. Statements (let, var, assignment, expression statements)
8. Patterns
9. Mixins (contract + provision) + associated types + GATs
10. Generics + bounds + where clauses
11. Variance
12. Drop + Copy + Clone
13. Derive
14. Concurrency (Send/Sync, Thread, Mutex, channels, atomics)
15. Async (Future, .await, executor, async I/O)
16. Module system + visibility (`private` / `protected` markers)
17. In-body directives (derive, include, layout, inline, deprecated,
    test, bench, etc.)
18. Macros (deferred — point at v2)
19. FFI (`lib` blocks, `layout c`, cbindgen)
20. Standard library overview
21. Editions + stability
22. Grammar (BNF/EBNF appendix)

## TDD

- Each chapter has at least one runnable code sample.
- A test harness `crates/riven-core/tests/reference_examples.rs`
  walks `docs/reference/**/*.md`, extracts ` ```riven ... ``` `
  blocks, compiles + runs each, asserts they succeed.
- Snippets that intentionally fail are tagged ` ```riven,fail `
  and asserted to produce a known error code.

## Implementation

- Use mdBook (`mdbook serve` for local preview).
- Grammar appendix uses EBNF (Open Decision #16: pick EBNF).
- Auto-generate the keyword/token list from `lexer/token.rs` via a
  build.rs.

## Definition of done

- [ ] All 22 chapters drafted with ≥1 example each.
- [ ] `cargo test reference_examples` runs every example.
- [ ] mdBook builds without warnings.
- [ ] Hosted at e.g. `riven-lang.org/reference/`.
- [ ] CHANGELOG bullet.
