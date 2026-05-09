# Tier 3.01 — LSP Enhancements

Status: draft
Depends on: none (Phase 1); doc 06 (incremental) for high-perf Phase 2+
Blocks: doc 03 (test) and doc 04 (doc) benefit from richer LSP for in-editor UX but do not hard-block

---

## 1. Summary & motivation

Today's `riven-lsp` implements five capabilities: `textDocument/didOpen`,
`didChange`, `didSave`, `hover`, `definition`, and `semanticTokens/full`.
A developer opening a Riven file in VSCode sees syntax highlighting
(TextMate grammar + semantic tokens), hover-for-types, goto-definition
*within a single file*, and error squigglies — but **only on save**, with
no diagnostics during editing. There is no completion, no
find-references, no rename, no inlay hints, no signature help, no code
actions, no LSP-driven formatting (though the formatter exists as a CLI),
no document symbols, and no workspace symbols. This is a functional but
minimal IDE experience; users who have seen rust-analyzer will find it
painfully sparse.

This doc specifies the v1 of a full-featured LSP: the capabilities to
ship, the `riven-ide` API surface they depend on, how incremental
analysis fits in, and the phasing that spreads the work over roughly
3-5 weeks.

---

## 2. Current state

### 2.1 LSP server capabilities (`crates/riven-lsp/src/server.rs`)

Declared in `initialize` at `server.rs:40-72`:

| Capability | Status | Location |
|---|---|---|
| `textDocumentSync` | Full-document sync, `save` fires `didSave`, `change` does not re-analyze | `server.rs:43-52`, `:109-123` |
| `hoverProvider` | Simple provider | `server.rs:53` |
| `definitionProvider` | Single-location; same-file only | `server.rs:54` |
| `semanticTokensProvider` | Full-document tokens, no range, no delta | `server.rs:55-67` |

Everything else on the LSP capability ladder is absent: no
`completionProvider`, no `renameProvider`, no `referencesProvider`, no
`documentSymbolProvider`, no `workspaceSymbolProvider`, no
`documentFormattingProvider`, no `documentRangeFormattingProvider`,
no `codeActionProvider`, no `inlayHintProvider`, no `signatureHelpProvider`,
no `documentHighlightProvider`, no `typeDefinitionProvider`,
no `implementationProvider`, no `foldingRangeProvider`,
no `selectionRangeProvider`.

### 2.2 Notification / request handlers in place

Only the ones the capabilities above imply, all in `server.rs`:

- `initialize` (`:40-72`)
- `initialized` (`:74-78`)
- `shutdown` (`:80-82`)
- `did_open` (`:84-107`) — runs full analysis + publishes diagnostics
- `did_change` (`:109-123`) — updates source buffer only, no re-analysis
- `did_save` (`:125-149`) — runs full analysis + publishes diagnostics
- `did_close` (`:151-159`) — clears diagnostics for URI
- `hover` (`:161-184`)
- `goto_definition` (`:186-214`)
- `semantic_tokens_full` (`:216-238`)

The analysis state is a `tokio::sync::RwLock<HashMap<Url, DocumentState>>`
(`server.rs:12-26`) — reasonable for Phase 1. Each `DocumentState` holds
the source, version, and the last `AnalysisResult` (which may be `None`
if no analysis has ever completed).

### 2.3 `riven-ide` semantic surface (`crates/riven-ide/src/`)

Seven modules, all consumed by the LSP:

| Module | Purpose | Key API |
|---|---|---|
| `analysis.rs` | Runs the full compiler pipeline against a source string | `pub fn analyze(source: &str) -> AnalysisResult` (`:43-88`) |
| `diagnostics.rs` | Converts `riven_core::diagnostics::Diagnostic` + `BorrowError` to LSP diagnostics | `collect_diagnostics` (`:63-75`) |
| `goto_def.rs` | Resolves cursor position to a single Location | `goto_definition` (`:8-39`) |
| `hover.rs` | Formats `Definition` + inferred type as markdown | `hover_at` (`:13-62`) |
| `line_index.rs` | Byte offset ↔ LSP UTF-16 position, span → range | `LineIndex::position_of` (`:26-41`) |
| `node_finder.rs` | HIR walk that returns the innermost node at a byte offset | `node_at_position` (`:16-23`) → `NodeAtPosition` enum (`:6-13`) |
| `semantic_tokens.rs` | Emits full-document semantic tokens (lexical + HIR-enriched) | `semantic_tokens` (`:39-58`) |

The `AnalysisResult` struct (`analysis.rs:13-21`) is the single point of
state: it holds the HIR program, symbol table, type context, raw
diagnostics, borrow errors, source, and a `LineIndex`. It is the natural
type to extend as LSP features grow.

Key limitations:

- Analysis is full-pipeline on every invocation. No memoization.
  `did_save` re-runs lex → parse → typeck → borrow-check every time.
- `AnalysisResult` has no reverse index from `DefId` to *use-sites*.
  Find-references would need this.
- `node_at_position` descends the HIR once per call. For repeated queries
  on the same document (hover + definition + references on one cursor
  position) it does redundant work. A position-indexed side-table would
  speed this up.
- `goto_def.rs:36` hard-codes `"file:///placeholder"` as the URI and the
  server overwrites it at `server.rs:210-213`. This is a Phase-1 hack
  that breaks the moment cross-file definition-jumping is supported.

### 2.4 Formatter availability

`riven_core::formatter::format(source: &str) -> FormatResult`
(`crates/riven-core/src/formatter/mod.rs:56-128`) and `format_range`
(`:131-134`) exist and are already exercised by the `rivenc fmt` CLI
(`crates/rivenc/src/main.rs:72-159`). Wiring LSP
`textDocument/formatting` to `format(source)` is ~20 lines. `format_range`
currently forwards to full-document formatting — range support is a
follow-up but declaring the capability is free.

### 2.5 VSCode client (`editors/vscode/src/extension.ts`)

Standard `vscode-languageclient/node` integration: launches the
`riven-lsp` binary over stdio, subscribes to `.riven` and `.rvn` files.
Accepts an override path via `riven.server.path` configuration
(`extension.ts:14-18`). No custom LSP extensions, no custom request
forwarding, no client-side quick-fix registration. The client is
capability-driven: adding a server capability auto-surfaces it in the
editor.

### 2.6 Symbol-table → LSP primitives

The following `DefKind` enum variants (`resolve/symbols.rs:70-128`) are
already distinguishable and therefore serve as the basis for document
symbols, workspace symbols, and the semantic-tokens mapping:

```
Variable, Function, Class, Struct, Enum, EnumVariant, Trait,
TypeAlias, Newtype, TypeParam, Module, Field, Method, Const,
Param, SelfValue
```

Each has a `span` for its declaration site. The symbol table iteration
API (`iter()` at `symbols.rs:201-203`) enumerates all definitions — the
basis for `workspace/symbol`.

---

## 3. Goals & non-goals

### Goals

1. A full LSP capability set matching what mid-sized languages (Zig's
   ZLS, OCaml's `ocaml-lsp-server`, Go's `gopls` phase 1) offer.
2. Diagnostics on every edit (debounced, incremental).
3. Deep integration with the Riven compiler's `HirProgram` +
   `SymbolTable`: no duplicated name-resolution logic in `riven-ide`.
4. Incremental-friendliness: the API shape in `riven-ide` should not
   change when doc 06 lands.
5. UTF-16 correctness for all positions (line_index already supports
   this — must not regress).
6. A clear phasing that delivers value every 3-5 days of work.

### Non-goals

- **Multi-file cross-references.** v1 ships same-project-wide
  definitions/references when the file is already open or discoverable
  via `riven-cli::module_discovery`. True workspace-wide indexing across
  unopened files is a stretch goal gated on doc 06.
- **Semantic tokens delta.** v1 emits full-document tokens; delta
  support ships later.
- **Macro-expansion display.** No macros ship today (tier1 doc 05).
  Once they do, expanding a macro in hover is a follow-up.
- **Rename across crates.** Rename is limited to the same file in v1,
  the same project in v2.
- **Debugger adapter wiring.** Doc 02 covers DAP.
- **LSP extension methods.** No custom `riven/*` request methods in
  v1. Only standard LSP.

---

## 4. Surface — capabilities, commands, configuration

### 4.1 Capabilities to declare in `initialize`

Every capability below has a matching request handler and a matching
`riven-ide` API.

| Capability | LSP method(s) | New? |
|---|---|---|
| `completionProvider` with `triggerCharacters: [".", ":", "("]` | `textDocument/completion`, `completionItem/resolve` | new |
| `signatureHelpProvider` with `triggerCharacters: ["(", ","]` | `textDocument/signatureHelp` | new |
| `documentSymbolProvider` | `textDocument/documentSymbol` | new |
| `workspaceSymbolProvider` | `workspace/symbol` | new |
| `renameProvider: { prepareProvider: true }` | `textDocument/rename`, `textDocument/prepareRename` | new |
| `referencesProvider` | `textDocument/references` | new |
| `documentHighlightProvider` | `textDocument/documentHighlight` | new |
| `codeActionProvider: { resolveProvider: true, codeActionKinds: ["quickfix", "refactor.rewrite"] }` | `textDocument/codeAction`, `codeAction/resolve` | new |
| `documentFormattingProvider` | `textDocument/formatting` | new |
| `documentRangeFormattingProvider` | `textDocument/rangeFormatting` | new |
| `inlayHintProvider: { resolveProvider: false }` | `textDocument/inlayHint` | new |
| `foldingRangeProvider` | `textDocument/foldingRange` | new |
| `selectionRangeProvider` | `textDocument/selectionRange` | new (nice-to-have) |
| `typeDefinitionProvider` | `textDocument/typeDefinition` | new |
| `implementationProvider` | `textDocument/implementation` | new |

### 4.2 Client-side settings (`editors/vscode/package.json` additions)

Extend the existing `riven.*` configuration block:

```json
"riven.inlayHints.typeHints": { "type": "boolean", "default": true },
"riven.inlayHints.parameterHints": { "type": "boolean", "default": true },
"riven.inlayHints.chainHints": { "type": "boolean", "default": false },
"riven.diagnostics.onEdit": { "type": "boolean", "default": true },
"riven.diagnostics.debounceMs": { "type": "number", "default": 200 },
"riven.completion.autoImport": { "type": "boolean", "default": true },
"riven.formatting.onSave": { "type": "boolean", "default": false },
```

These are client-side hints; the server reads them via
`workspace/configuration`.

### 4.3 Debounce + cancellation

All "on edit" work (diagnostics, completion request that races an edit)
must be debounced. Target: 200 ms after the last `didChange`.
Outstanding requests for stale document versions return
`LspError::code(RequestCancelled)`.

---

## 5. Architecture / design

### 5.1 `riven-ide` API extensions

Each new LSP feature maps to one new public function in `riven-ide`.
The signatures below are the target — server handlers are thin adapters
that convert LSP types to these and back.

```rust
// crates/riven-ide/src/completion.rs (new)
pub fn completions(result: &AnalysisResult, position: lsp_types::Position,
                   trigger: Option<char>) -> Vec<CompletionItem>;

// crates/riven-ide/src/signature_help.rs (new)
pub fn signature_help(result: &AnalysisResult, position: lsp_types::Position)
                      -> Option<SignatureHelp>;

// crates/riven-ide/src/document_symbols.rs (new)
pub fn document_symbols(result: &AnalysisResult) -> Vec<DocumentSymbol>;

// crates/riven-ide/src/workspace_symbols.rs (new)
pub fn workspace_symbols(results: &[(Url, &AnalysisResult)], query: &str)
                         -> Vec<SymbolInformation>;

// crates/riven-ide/src/references.rs (new)
pub fn references(result: &AnalysisResult, position: lsp_types::Position,
                  include_decl: bool) -> Vec<lsp_types::Location>;

// crates/riven-ide/src/rename.rs (new)
pub fn prepare_rename(result: &AnalysisResult, position: lsp_types::Position)
                      -> Option<lsp_types::Range>;
pub fn rename(result: &AnalysisResult, position: lsp_types::Position,
              new_name: &str) -> Option<WorkspaceEdit>;

// crates/riven-ide/src/highlight.rs (new)
pub fn document_highlights(result: &AnalysisResult, position: lsp_types::Position)
                           -> Vec<DocumentHighlight>;

// crates/riven-ide/src/code_actions.rs (new)
pub fn code_actions(result: &AnalysisResult, range: lsp_types::Range,
                    context: &CodeActionContext) -> Vec<CodeAction>;

// crates/riven-ide/src/inlay_hints.rs (new)
pub fn inlay_hints(result: &AnalysisResult, range: lsp_types::Range,
                   cfg: &InlayHintConfig) -> Vec<InlayHint>;

// crates/riven-ide/src/folding.rs (new)
pub fn folding_ranges(result: &AnalysisResult) -> Vec<FoldingRange>;

// crates/riven-ide/src/format.rs (new) — thin wrapper over riven_core::formatter
pub fn format_document(source: &str) -> Option<Vec<TextEdit>>;
pub fn format_range(source: &str, range: lsp_types::Range) -> Option<Vec<TextEdit>>;

// crates/riven-ide/src/type_def.rs (new)
pub fn type_definition(result: &AnalysisResult, position: lsp_types::Position)
                       -> Option<lsp_types::Location>;
```

Adding ~14 new modules is fine — `riven-ide` is currently 7 modules and
~700 lines. Each new module owns a ~60-200 line implementation.

### 5.2 Reverse-index for references, rename, highlight

The single most important data structure that `riven-ide` lacks today is
a `DefId -> Vec<Span>` map of *use-sites*. `goto_def.rs` walks forward
(reference → definition); references/rename/highlight need the opposite.

Add to `AnalysisResult`:

```rust
pub struct UseIndex {
    // All references to each DefId (includes the definition span as the first entry).
    pub uses: HashMap<DefId, Vec<Span>>,
}
```

Build it once, during `analyze()`, by walking the HIR the same way
`node_finder` does. Cost: one extra HIR traversal per analyze; O(n) in
number of nodes. Feasible within the 50 ms analyze budget.

### 5.3 Diagnostics-on-edit (debouncing)

Change `did_change` to:

1. Update the document source in-memory (as today).
2. Increment a per-document `pending_analysis_gen: u64` counter.
3. Spawn a `tokio::spawn` that waits 200 ms then checks whether the gen
   still matches. If so, run `analyze()` and publish diagnostics.
4. If another `did_change` arrives before the timer fires, it bumps the
   generation and the in-flight task exits without work.

This debounced-generation pattern avoids lock contention and
spurious double-analysis. Reference implementations:
[rust-analyzer's `GlobalState::maybe_switch_configuration`] style pattern.

Watch out for two things:

- Analysis is not cancellable mid-run. If a 500 ms analyze is underway
  and three edits arrive, the user sees 500 ms stale diagnostics. Doc
  06 (incremental) fixes this by making analysis cheap; Phase 1 just
  accepts the staleness.
- `tower-lsp` runs handlers concurrently, but internal state behind
  `RwLock` can block. Keep critical sections short.

### 5.4 Completion — phased by trigger

Completion is the most complex capability. Split into four triggers:

1. **Word-start (after any identifier character).** Offer:
   - Every in-scope local / param / function / module.
   - Every type in the current module's type registry.
   - Every built-in keyword that fits the context (heuristically, see below).
2. **After `.`.** Offer:
   - Fields of the receiver's resolved type.
   - Methods defined on the receiver's type (including via trait impls).
   - Built-in methods from `typeck::infer::builtin_method_type` for
     primitive / stdlib types.
3. **After `::` (if we adopt Rust-style paths) or after a module
   qualifier.** Offer module contents.
4. **After `(` or `,` inside a call.** Offer signature help instead
   of completion, but also filter completion by the expected parameter
   type.

Context heuristics for keywords: after `def` suggest nothing (the user
is typing a function name); after `if` suggest nothing; after `end`
suggest the containing keyword (`class`/`def`/`if`). For Phase 1, do not
over-engineer: keywords always appear, ranked low.

Completion sorting: use `sortText` to rank (1) exact prefix match,
(2) case-insensitive prefix match, (3) substring match, (4) everything
else. Ignore fuzzy matching in Phase 1 (the client does its own).

### 5.5 Inlay hints

Two kinds for v1:

1. **Type hints on unannotated `let`.** Walk every `HirStatement::Let`
   where the source text at `pattern.span` is a bare identifier (no
   explicit `: T`). Render `: InferredType` as a hint trailing the
   pattern.
2. **Parameter name hints at call sites.** For each `HirExprKind::FnCall`
   / `MethodCall`, look up the callee `FnSignature`, and emit a hint
   `param_name:` before each argument. Skip when the argument's source
   text is already an identifier equal to the parameter name (Rust
   convention).

Hint position uses `Span` end-of-pattern and argument-start, mapped via
`LineIndex`. Resolver is unused — hints have no resolve step in v1.

### 5.6 Rename

Implementation:

1. `prepare_rename` — return `Some(range)` if the cursor is on a
   `NodeAtPosition::{VarRef, Definition, MethodCall, FnCall}`, else
   `None`.
2. `rename(new_name)` — resolve the cursor to a `DefId`, look up
   `UseIndex.uses[def_id]`, return a `WorkspaceEdit` with one `TextEdit`
   per use-site, every edit replacing `Span` → `new_name`.
3. Validate `new_name` as a Riven identifier before returning the edit
   (must match `[a-z_][a-zA-Z0-9_]*` for value bindings; `[A-Z][a-zA-Z0-9_]*`
   for types). Return `Err` for an invalid name.

Rename across files is v2. Rename of a public symbol used by a
dependency's `.rlib` is out of scope.

### 5.7 Code actions / quick fixes

Phase 1 set (low-hanging):

- **`unused variable`** → prefix with `_`.
- **`missing import`** — once stdlib lands with a real module surface.
  Scan top-level `use` decls for a matching unimport; offer insertion.
- **`add missing semicolon`** — not applicable; Riven is
  newline-terminated.
- **`wrap in Some(...)` / `Ok(...)`** when the return type expects one.
- **`add `&`` / `add &mut`** when a borrow-checker error suggests it.

Each quick-fix reads a `Diagnostic.code` (e.g. `"E0001"`) or a
`BorrowError.code` and dispatches to a matching fixer. The dispatcher
is a match on the code string.

### 5.8 Formatting

Two handlers:

```rust
async fn formatting(params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>>
async fn range_formatting(params: DocumentRangeFormattingParams) -> Result<Option<Vec<TextEdit>>>
```

Both call `riven_core::formatter::format` / `format_range` on the
in-memory source and produce a single whole-document `TextEdit` that
replaces the entire range with the formatted output. No diffing — the
client applies the replacement atomically.

Config options (optional, client-provided via `workspace/configuration`):
`riven.formatting.tabWidth`, `riven.formatting.useSpaces`. Today the
formatter has no knobs (`formatter/mod.rs` takes no options); add them
later.

Cost estimate: ~30 lines in `riven-ide/src/format.rs` + ~20 lines of
handler boilerplate. **Very cheap first win.**

---

## 6. Implementation plan

### Files to touch

| Area | Crate | File | Change |
|---|---|---|---|
| Server caps | `riven-lsp` | `src/server.rs` | Extend `ServerCapabilities` in `initialize`; add handlers |
| Debounce | `riven-lsp` | `src/server.rs` | Add `pending_gen: AtomicU64` per doc; use `tokio::spawn + sleep` |
| IDE API | `riven-ide` | `src/lib.rs` | Export 14 new modules |
| IDE API | `riven-ide` | `src/completion.rs` *new* | Scope-aware completion |
| IDE API | `riven-ide` | `src/signature_help.rs` *new* | Active-param tracking |
| IDE API | `riven-ide` | `src/document_symbols.rs` *new* | Walk HIR top-level items |
| IDE API | `riven-ide` | `src/workspace_symbols.rs` *new* | Query across `HashMap<Url, AnalysisResult>` |
| IDE API | `riven-ide` | `src/references.rs` *new* | Uses `UseIndex` |
| IDE API | `riven-ide` | `src/rename.rs` *new* | Uses `UseIndex` |
| IDE API | `riven-ide` | `src/highlight.rs` *new* | Same-document references |
| IDE API | `riven-ide` | `src/code_actions.rs` *new* | Diagnostic-driven |
| IDE API | `riven-ide` | `src/inlay_hints.rs` *new* | Type hints + param name hints |
| IDE API | `riven-ide` | `src/folding.rs` *new* | Class/struct/def/`if`/`match` ranges |
| IDE API | `riven-ide` | `src/format.rs` *new* | Thin wrapper over formatter |
| IDE API | `riven-ide` | `src/type_def.rs` *new* | "Go to type definition" |
| Index | `riven-ide` | `src/analysis.rs` | Add `UseIndex`, build during `analyze()` |
| Node finder | `riven-ide` | `src/node_finder.rs` | Add more node kinds (pattern binding, import path) |
| Tests | `riven-ide` | `tests/integration.rs` | Add integration tests per feature (see §10) |
| Client settings | `editors/vscode` | `package.json` | Add `riven.inlayHints.*`, `riven.diagnostics.*` |
| Client init | `editors/vscode` | `src/extension.ts` | Wire `workspace/configuration` response |

### Per-phase file deltas

**Phase 1A (diagnostics-on-edit + formatting, 3 days):**
- `server.rs:109-123` (`did_change`) rewrite: debounced re-analyze.
- `server.rs` add `documentFormattingProvider` / `documentRangeFormattingProvider`
  capabilities + handlers.
- `riven-ide/src/format.rs` new.
- Tests: `tests/format_integration.rs`, `tests/debounce_integration.rs`.

**Phase 1B (completion, 5-7 days):**
- `riven-ide/src/completion.rs` new, ~400 lines.
- `server.rs` add completion handler + capability.
- Tests: a dozen fixtures for `foo.` / `Bar::` / scope-based completion.

**Phase 1C (symbols + references + rename, 5 days):**
- `riven-ide/src/analysis.rs` add `UseIndex`.
- `riven-ide/src/document_symbols.rs`, `workspace_symbols.rs`,
  `references.rs`, `rename.rs`, `highlight.rs`, `type_def.rs` new.
- `server.rs` add 6 handlers + capabilities.
- Tests per capability.

**Phase 1D (inlay hints + signature help + folding + code actions, 5 days):**
- `riven-ide/src/inlay_hints.rs`, `signature_help.rs`, `folding.rs`,
  `code_actions.rs` new.
- `server.rs` add 4 handlers + capabilities.
- Tests per capability.

**Phase 2 (after doc 06 lands):**
- Replace `analyze()` full-pipeline with `analyze_incremental()` against
  the query layer. Each LSP handler signature is unchanged.
- Move to `semanticTokens/full/delta`.
- Cross-file references/rename via a workspace-scoped index.

---

## 7. Interactions with other tier-3 items

- **Doc 06 (incremental).** LSP is the largest consumer of incremental
  analysis. The `AnalysisResult` → query-layer migration must preserve
  every field LSP reads from. Plan: doc 06 lands with a shim that
  exposes `analyze_incremental(...) -> AnalysisResult` with the same
  shape as today, so LSP handlers are unchanged.
- **Doc 04 (doc generator).** LSP hover should render the associated
  `##` doc comment of the hovered symbol. Requires doc 04's AST/HIR
  change (stop discarding doc comments in the parser). Cheap once that
  lands: add a `doc_comment: Option<String>` field to `Definition`
  (`resolve/symbols.rs:140`) and render it in `hover.rs:75` under the
  signature.
- **Doc 03 (test framework).** LSP code-lens ("Run test", "Debug test")
  above each `@[test]` function. Phase 1.5 — after the test framework
  ships.
- **Doc 02 (debugger).** LSP → DAP handoff. Not in scope for this doc;
  the debug experience lives in the editor's debug UI, which consumes
  DAP directly.
- **Tier-1 doc 05 (derive).** LSP completion for `@[derive(...)]`
  argument list needs to know the set of valid derivable traits. Once
  tier-1 doc 05 lands, add a special case in completion's `.rs:context_for_attr_arg`.

---

## 8. Phasing

**Phase 1A — formatting + diagnostics-on-edit (3 days).**
Wins that take a day each. Validates that the `tower-lsp` plumbing for
new capabilities works, and gives users immediate visible value.

**Phase 1B — completion (5-7 days).**
The hardest single-feature. Do not under-estimate. Bucket: scope
completion (2 days), field/method completion (2 days), keyword
completion + sort order (1 day), completion item resolve (1 day).

**Phase 1C — symbols + references + rename (5 days).**
The `UseIndex` is the prerequisite for three features. Build the index
first; the three features are then ~1 day each. Add a fourth day for
edge cases (shadowed variables, parameter rename across closures).

**Phase 1D — hints + signature help + folding + code actions (5 days).**
Low-risk, each feature is ~1 day. Quick-fixes take the bulk — each
fix kind needs its own dispatcher.

**Phase 2 — post-incremental migration (2 days).**
Follow doc 06. Re-run the LSP test suite to verify no regressions.

**Phase 3 — cross-file features (open-ended).**
Workspace symbols over the whole project, rename across files,
find-references across files. Depends on the compiler gaining
multi-file compilation beyond the current concat-sources model
(`riven-cli/src/build.rs` `gather_sources`) and on `riven-ide` gaining
a workspace manager that watches `src/**.rvn`.

Total: 18-22 engineer-days for Phases 1A-1D.

---

## 9. Open questions & risks

1. **Q1 — When a file has parser errors, what completion do we show?**
   Today `analyze()` returns early on parse errors with `program: None`.
   Completion falls back to lexical-only (keywords + recent tokens).
   Alternative: parser error recovery (emits a partial AST). The former
   is simple; the latter gives a much better experience. Call.
2. **Q2 — Completion in comment/string positions.** Disable or still show
   identifiers? rust-analyzer disables. Recommendation: disable.
3. **Q3 — Find references without loading every file.** Today references
   only work inside the currently-open document. A workspace-wide index
   requires opening every `.rvn` file in the project. Plan: compute
   lazily on first `workspace/symbol`, cache per-file; invalidate on
   `didChangeWatchedFiles`. This adds memory pressure; cap at ~10k
   symbols.
4. **R1 — `tower-lsp` cancellation.** `tower-lsp` 0.20 doesn't expose a
   great cancellation model. We may need to thread a `CancellationToken`
   manually through each handler. Alternative: switch to `lsp-server`
   (crates.io) for lower-level control. Defer.
5. **R2 — Debounce correctness with rapid typing.** If a user types
   `a`, `b`, `c` in 50 ms increments, the debounce must fire on `c`
   only. Testing this needs a deterministic clock abstraction. Use
   `tokio::time::pause` in tests.
6. **R3 — UTF-16 column drift.** The `LineIndex` already handles
   UTF-16 code-unit columns correctly (see the `emoji_utf16_surrogate_pair`
   test at `line_index.rs:120-126`). Every new feature that reports a
   `Range` must go through `LineIndex::span_to_range` — no shortcuts.
7. **R4 — Parameter-name inlay hint when closure params shadow outer vars.**
   Annoying but solvable; the hint machinery must respect lexical scope.
8. **R5 — Format-on-save conflict with edits-in-flight.** If the user
   edits while a format is mid-flight, the client's `workspace/applyEdit`
   may land on stale text. Follow LSP convention: fail `applyEdit` with
   a friendly message.
9. **Q4 — Completion item sort.** Should we do frecency (most-recently
   used first)? Worth it for Phase 2, not Phase 1. Requires per-user
   state; skip for v1.
10. **Q5 — Per-client configuration surface stability.** Are the
    `riven.inlayHints.*` / `riven.diagnostics.debounceMs` names we want
    to lock in? These become a compat surface. Decide before merging.

---

## 10. Test matrix

LSP work benefits from end-to-end tests driven by the `tower-lsp` test
client plus unit tests on `riven-ide` pure functions.

| Feature | Fixture file | Assertions |
|---|---|---|
| Diagnostics-on-edit | `tests/lsp_did_change.rs` | Publish after 200 ms; cancel in-flight on new edit |
| Formatting | `tests/format_lsp.rs` | Whole-file + range; idempotent on already-formatted input |
| Completion: scope | `tests/completion_scope.rs` | `let x = 1\n  let y = _\n` offers `x` |
| Completion: field | `tests/completion_field.rs` | After `.`, show fields then methods |
| Completion: trigger | `tests/completion_trigger.rs` | `.` triggers, `,` inside call doesn't |
| Signature help | `tests/signature_help.rs` | Active param index tracks comma position |
| Document symbols | `tests/document_symbols.rs` | Hierarchical; class contains methods |
| Workspace symbols | `tests/workspace_symbols.rs` | Prefix match across files |
| References | `tests/references.rs` | Include decl when `includeDeclaration: true` |
| Rename | `tests/rename.rs` | Returns WorkspaceEdit covering every use; validates identifier |
| Prepare rename | `tests/prepare_rename.rs` | Returns span of name only, not whole statement |
| Document highlight | `tests/highlight.rs` | All same-symbol uses in active doc |
| Inlay hints: types | `tests/inlay_types.rs` | Present on unannotated `let`, absent on annotated |
| Inlay hints: params | `tests/inlay_params.rs` | Present for fn call; skipped when arg name == param name |
| Folding ranges | `tests/folding.rs` | `class`, `def`, `match`, multi-line comments |
| Code action: unused var | `tests/code_action_unused.rs` | Offers `_`-prefix fix |
| UTF-16 column | `tests/utf16_columns.rs` | Emoji + accented chars correct |

Each test should exercise the full `initialize` → `didOpen` → request →
response flow rather than calling `riven-ide` functions directly. This
catches capability-declaration mistakes.

Additionally, add a "LSP smoke test" that spawns `riven-lsp` as a
subprocess, sends a canned `initialize`, then disconnects — verifies
the binary starts without panicking. Integrate into CI alongside the
existing `installed_binary` test pattern.
