# 20 — Phase 5: rivendoc (T3.04)

**Depends on:** P0.13 doc comments wired (✅ already done) + Phase 2
stdlib (so generated docs have content).
**Reads:** `docs/requirements/tier3_04_doc_generator.md`.

## Goal

`riven doc` generates static HTML from `##` doc comments captured at
parse time. Output matches `cargo doc` ergonomics: per-item pages,
sidebar nav, full-text search.

## TDD

- Unit test: `rivendoc::extract(program)` returns one `DocItem` per
  documented HIR item.
- Unit test: HTML emit produces well-formed HTML with `<h1>`,
  `<pre><code>`, etc.
- E2E test: run `riven doc` on a fixture project; assert
  `target/doc/index.html` exists with expected entries.
- Test that doc-comment markdown (code fences, links, headings)
  renders correctly.

## Implementation

- New crate `crates/rivendoc/`.
- Walks `HirProgram`, harvests the Rust-side `doc_comments` field
  (a `Vec<String>` on the internal HIR node) from every public item.
- Markdown → HTML via `pulldown-cmark`.
- Templates via `tera` or hand-rolled (avoid heavy dep).
- Search: build a flat JSON index, ship a small JS file for
  client-side search.

## CLI

- `riven doc` — build HTML for current project.
- `riven doc --open` — open in browser after build.
- `riven doc --no-deps` — skip dependencies.

## Definition of done

- [ ] `riven doc` works on the in-tree examples and produces a
      navigable static site.
- [ ] Code blocks in doc comments are syntax-highlighted.
- [ ] Search returns results for symbol names.
- [ ] Generated site is committed under `target/doc/` for the
      stdlib so the website can host it.
- [ ] CHANGELOG bullet.
