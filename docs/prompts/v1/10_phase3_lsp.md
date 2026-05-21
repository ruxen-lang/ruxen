# 10 — Phase 3: LSP server (T3.01) — basic completion + diagnostics

> **Status: 🟡 Partial** (audited 2026-05-21). LSP server in
> `src/riven_lsp/{src,tests}/` — diagnostics + integration tests
> shipped. Full coverage of hover, goto-def, completions,
> code-actions needs audit against `tier3_01_lsp.md`.

**Depends on:** Phase 2 stdlib stable.
**Reads:** `docs/requirements/tier3_01_lsp.md`,
`src/riven_lsp/` (no longer the empty shell — server + tests in tree).

## Phase 1 scope (this prompt)

Ship a working LSP that supports:

- `textDocument/didOpen`, `didChange`, `didClose`
- `textDocument/publishDiagnostics` (full-file re-analysis on each
  change, debounced 250ms)
- `textDocument/hover` — show type + doc-comment for symbol at cursor
- `textDocument/definition` — go-to-def
- `textDocument/completion` — keywords + in-scope identifiers + type
  members
- `textDocument/formatting` — wire existing `formatter` crate

Defer to phase 2 (separate prompt 11): incremental analysis,
`workspace/symbol`, code actions, rename.

## TDD

Write integration tests in `crates/riven-lsp/tests/` that drive the
server via the LSP JSON-RPC protocol. For each capability:

1. Initialize handshake test (`initialize` → expected
   `capabilities`).
2. didOpen + diagnostics — open a file with a known type error, assert
   `publishDiagnostics` carries the right code and span.
3. Hover on identifier → expected `MarkupContent` containing type +
   doc.
4. Goto-def test — open project with two files, request
   definition, assert response location.
5. Completion at known cursor — assert `CompletionItem` list contains
   expected names.
6. Formatting test — request formatting, compare against
   `formatter::format()` output.

Use `tower-lsp` if not already wired; spawn the server in-process and
talk to it via channels.

## Implementation

- Reuse `riven-core` for parse / typecheck / formatter.
- Each `didChange` re-runs the full pipeline on the changed file
  with a 250ms debounce. Acceptable for v1; prompt 11 wires the
  query layer.
- Diagnostic mapping: `Diagnostic.span` → LSP `Range` (1-indexed
  line, 0-indexed char).
- Hover: walk HIR to find the def at cursor; return `Type` and
  `doc_comments` (P0.13 captured them).
- Completion: collect every name in `SymbolTable` whose scope
  encloses the cursor. Filter by prefix.

## Editor wiring

Add `editors/vscode/` extension stub that launches the LSP and
configures association with `*.rvn`. Document in `editors/README.md`.

## Reserved error codes

- E1200 — LSP request type mismatch
- E1201 — workspace root not found

## Definition of done

- [ ] LSP server binary `rivenlsp` builds.
- [ ] All 6 capabilities pass integration tests.
- [ ] VSCode extension stub launches the server on `*.rvn` open.
- [ ] CI green.
- [ ] CHANGELOG bullet.
