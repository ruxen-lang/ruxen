# Tier 3.04 — Doc Generator (`rivendoc`)

Status: draft
Depends on: none (but LSP doc 01 benefits from the same AST change)
Blocks: self-hosted documentation of the stdlib (tier-1 doc 01)

---

## 1. Summary & motivation

Riven has no way to produce documentation. Users who comment their
code with `## This function adds two integers` see their words
extracted by the lexer as `TokenKind::DocComment` (`lexer/mod.rs:211-226`)
and then silently discarded at four places in the parser:
`parser/mod.rs:455-458`, `:884-887`, `:1112-1115`, `:1187-1190`. The
formatter round-trips them by re-emitting from the token stream
(`formatter/format_expr.rs:442`), but nothing semantic consumes them.

No `rivendoc` binary exists. No HTML template. No search index. No
cross-reference resolution. The consequence: once Tier-1 stdlib lands,
users will have ~20 new modules and types with no reference documentation
except tutorial prose.

This doc specifies:

1. Capturing `##` doc comments on every documentable item
   (functions, methods, classes, structs, enums, variants, traits,
   modules, type aliases, newtypes, consts, fields).
2. A `rivendoc` binary that produces an HTML reference site from a
   project's source, including cross-links, a search index, and
   Markdown rendering of doc-comment bodies.
3. A path to JSON output for consumption by other tools (IDE hover,
   search engines, etc.).

---

## 2. Current state

### 2.1 Doc-comment syntax already decided

The lexer recognizes **`##`** as the doc-comment syntax
(`crates/riven-core/src/lexer/mod.rs:170-172`, `:211-226`):

```rust
} else if self.peek_at(1) == Some('#') {
    // Doc comment ##
    self.lex_doc_comment(start_byte, start_line, start_col);
}
```

This fires when a line starts with `##` (two hashes). A single `#`
is a regular line comment. The content between `##` and newline is
captured into `TokenKind::DocComment(String)` (`lexer/token.rs:217`).
There is a one-character leading-space strip (`lexer/mod.rs:216-218`).

Block comments use `#= ... =#` (`lexer/mod.rs:182-209`).
There is no block doc-comment syntax today. Options for blocks:

- `#| ... |#` — mirror `#=` visually.
- Keep `##` line-only (like Rust `///`) and leave block doc comments
  as a v2 item.

**Recommend keeping `##` line-only for v1.** Matches the tutorial-prose
style authors already use, avoids an ambiguity with regular block
comments.

### 2.2 Doc comments are discarded in the parser

Every site that expects a new item skips any leading `DocComment`s:

```rust
// parser/mod.rs:455-458
while let TokenKind::DocComment(_) = self.current_kind() {
    self.advance();
    self.skip_newlines();
}
```

Same pattern at `:884-887`, `:1112-1115`, `:1187-1190`. The text is
never captured on the AST. `parse_top_level_item`, `parse_class_def`,
`parse_func_def`, and `parse_struct_def` all run this loop.

### 2.3 Formatter round-trips by re-emitting from tokens

The formatter preserves doc comments (`formatter/format_expr.rs:442`:
`TokenKind::DocComment(s) => format!("## {}", s)`), but it does so by
reading them out of the token stream directly rather than from any AST
node. That means if we start attaching them to AST nodes, we must
either update the formatter to read from AST nodes, or keep the
token-based path and accept some duplication.

### 2.4 No `rivendoc` binary, no HTML output, no search

A grep for `rivendoc` in `crates/` returns zero matches. No templating
engine, no Markdown parser, no search-index builder.

### 2.5 Existing public surface to document

Once tier-1 stdlib lands, the documented items are the exported set.
Visibility levels already exist (`ast::Visibility::{Public, Protected,
Private}`) and are tracked on `Definition.visibility`
(`resolve/symbols.rs:144`). `rivendoc` defaults to documenting
`Public` items only.

---

## 3. Goals & non-goals

### Goals

1. `rivendoc` binary that produces a navigable HTML site from
   `src/**.rvn` in a project.
2. Every public item in the source has a dedicated page (or section)
   with its doc comment rendered.
3. Cross-references: `see [Vec.push]` in a doc comment renders as a
   link to the `Vec.push` method page.
4. A fuzzy-searchable index page (client-side JS, single JSON blob).
5. Markdown formatting inside doc comments (bold, code blocks, lists,
   links).
6. `## ```rvn` code blocks get Riven syntax highlighting.
7. Works against a single file (`rivendoc path/to/file.rvn`) and a
   project (`rivendoc` in the project root).
8. Doc comments surface in LSP hover (shared work with doc 01).
9. Optional JSON output (`rivendoc --format=json`) for IDE/search
   tooling.

### Non-goals

- **User-configurable theme.** One built-in theme; v2 adds themes.
- **API diffing between versions.** "What changed in my public API"
  is a v2+ concern.
- **Inter-package links** (i.e. link from `my-app` docs to
  `std` docs). v2.
- **Doctest execution.** Running code blocks as tests (like Rust's
  rustdoc) is a stretch goal; defer.
- **Private-item docs.** `--document-private` flag v2.
- **PDF, EPUB, or other output formats.** HTML + JSON only.
- **Internationalization.** English only.

---

## 4. Surface

### 4.1 Syntax for authors

```
## Adds two integers.
##
## Panics if the addition overflows.
##
## # Examples
##
## ```rvn
## let result = add(2, 3)
## assert_eq(result, 5)
## ```
##
## See also: [sub], [Vec.push]
def add(a: Int, b: Int) -> Int
  a + b
end
```

Rules:

- A doc comment is a contiguous run of `##` lines immediately preceding
  an item (no blank lines between).
- Markdown syntax is rendered via a CommonMark subset.
- `[Name]` or `[Module.Name]` resolves to a cross-link.
- Fenced code blocks with language tag `rvn` / `riven` get syntax
  highlighted.

### 4.2 CLI

```
rivendoc                             # docs for project at cwd
rivendoc src/main.rvn                # docs for a single file
rivendoc --output docs/              # output directory (default: target/doc/)
rivendoc --format=html               # default
rivendoc --format=json               # single JSON blob
rivendoc --no-deps                   # don't rebuild dep docs
rivendoc --open                      # after build, xdg-open / open the index.html
riven doc                            # forwards to rivendoc (see §4.3)
riven doc --open
```

### 4.3 `riven doc` subcommand

Add to `crates/riven-cli/src/cli.rs::Command`:

```rust
Doc {
    #[arg(long)] open: bool,
    #[arg(long = "no-deps")] no_deps: bool,
    #[arg(long)] release: bool,
    #[arg(long)] format: Option<String>,
},
```

Implementation: shell out to `rivendoc` binary (parallel to
`rivenc`/`riven-lsp`/`riven-dap`).

### 4.4 Output layout

```
target/doc/
├── index.html                # project overview + module list
├── search-index.json         # fuzzy search data
├── style.css
├── script.js                 # client-side search
├── my_app/
│   ├── index.html            # module page
│   ├── struct.Foo.html       # one page per top-level item
│   ├── fn.bar.html
│   └── trait.Baz.html
└── std/                      # if --document-deps (v2), dep docs live here
```

One HTML per item keeps URLs stable and shareable. Module pages link
to their items; item pages link back to modules via breadcrumbs.

### 4.5 JSON format (for tooling)

```json
{
  "version": "1",
  "project": "my-app",
  "items": [
    {
      "path": "my_app::math::add",
      "kind": "function",
      "visibility": "public",
      "signature": "def add(a: Int, b: Int) -> Int",
      "doc": "Adds two integers.\n\nPanics if the addition overflows.",
      "span": { "file": "src/math.rvn", "line": 12, "col": 1 },
      "links": ["my_app::math::sub", "std::Vec::push"]
    }
  ]
}
```

---

## 5. Architecture / design

### 5.1 AST change — attach doc comments to items

New field `doc_comments: Vec<String>` on every documentable AST node:

- `ast::FuncDef`
- `ast::ClassDef`, `ast::StructDef`, `ast::EnumDef`, `ast::TraitDef`
- `ast::ImplBlock` (for impl-level docs; rare but useful)
- `ast::ModuleDef`
- `ast::TypeAlias`, `ast::NewtypeDef`
- `ast::ConstDef`
- `ast::EnumVariant`
- `ast::FieldDef`

Then update the four parser sites that currently discard:

```rust
// parser/mod.rs:455-458 — was:
while let TokenKind::DocComment(_) = self.current_kind() {
    self.advance();
    self.skip_newlines();
}

// becomes:
let mut doc_comments = Vec::new();
while let TokenKind::DocComment(s) = self.current_kind() {
    doc_comments.push(s.clone());
    self.advance();
    // Do not skip newlines — that breaks contiguous-doc detection.
    // Instead, only skip the single newline that follows a DocComment.
    if matches!(self.current_kind(), TokenKind::Newline) {
        self.advance();
    }
}
// Attach doc_comments to whatever item parses next.
```

Propagate into HIR: add `doc_comments: Vec<String>` to every matching
HIR node (`HirFuncDef`, `HirStructDef`, etc. in `hir/nodes.rs`).

And into `Definition`:

```rust
// resolve/symbols.rs:140-146
pub struct Definition {
    pub id: DefId,
    pub name: String,
    pub kind: DefKind,
    pub visibility: Visibility,
    pub span: Span,
    pub doc_comments: Vec<String>,  // NEW
}
```

This single-field addition unblocks both `rivendoc` and LSP-hover
doc rendering (doc 01 §7).

### 5.2 `rivendoc` binary

New crate `crates/rivendoc/`:

```
crates/rivendoc/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── collect.rs       # walk project, build item list
│   ├── markdown.rs      # doc-comment body → HTML
│   ├── xref.rs          # [Name] → URL resolution
│   ├── search.rs        # build search-index.json
│   ├── html/
│   │   ├── mod.rs       # template dispatch
│   │   ├── module.rs
│   │   ├── function.rs
│   │   ├── class.rs
│   │   ├── struct.rs
│   │   ├── enum.rs
│   │   └── trait.rs
│   └── assets/
│       ├── style.css
│       └── script.js
└── tests/
    └── snapshot.rs
```

Dependencies: `pulldown-cmark` for Markdown, `serde_json` for search
index. No templating engine — small enough to hand-roll with `format!`.
(Rust's rustdoc uses `minijinja` but we don't need that scale.)

### 5.3 Item collection

Reuse `riven_core::{lexer, parser, resolve, typeck}`. After `typeck`,
`TypeCheckResult.symbols` has every definition with its span and
(after §5.1) doc comments. Walk `symbols.iter()` and produce:

```rust
struct CollectedItem {
    path: String,         // "my_app::math::add"
    kind: ItemKind,       // Function, Class, Struct, Enum, ...
    visibility: Visibility,
    signature: String,    // pretty-printed signature
    doc: String,          // joined doc_comments
    span: Span,
    file: PathBuf,
}
```

Only `Visibility::Public` items are included by default.

### 5.4 Cross-reference resolution

When rendering doc-comment Markdown, intercept link-like spans that
match `[Name]` / `[A.B.C]`. Resolution:

1. If the fragment is `A.B`, try `symbols.lookup_path(["A", "B"])`.
2. On match, rewrite the link to the item's HTML URL.
3. On miss, leave as plain text but emit a warning: `warning:
   unresolved doc link [A.B] at src/foo.rvn:12`.

The symbol-table path-lookup API doesn't exist today — it's one helper
function on `SymbolTable` iterating the definitions vector.

### 5.5 Search index

`search-index.json` is a flat array of `{name, path, kind, summary}`
entries. Client-side JS (vanilla, no framework; target ~2 KB gzipped)
does `String.prototype.includes()` substring match — good enough for
100s-1000s of items. Algorithm upgrade (fuzzy scoring) is easy later.

### 5.6 Markdown subset

Use `pulldown-cmark` with:
- Headings (auto-linked with anchor IDs for in-page jumps)
- Paragraphs
- Lists (ordered, unordered)
- Code spans (`` `foo` ``)
- Fenced code blocks with language tag
- Bold, italic, strikethrough
- Links
- Tables (`pulldown-cmark` option)

Disable: raw HTML, autolinks, footnotes (can be re-enabled later).

### 5.7 Syntax highlighting for `rvn` code blocks

Reuse `riven_core::lexer::Lexer` to tokenize the code, then emit HTML
spans:

```rust
fn highlight(code: &str) -> String {
    let mut out = String::new();
    let mut lexer = Lexer::new(code);
    let tokens = lexer.tokenize().unwrap_or_default();
    for tok in tokens {
        let class = css_class_for(&tok.kind);
        write!(out, "<span class=\"{}\">{}</span>", class, escape(&tok.text())).unwrap();
    }
    out
}
```

The same token-to-class mapping can reuse `riven-ide/src/semantic_tokens.rs:87-161`
logic.

### 5.8 Module/item URL scheme

```
crate-path/module/module/item.html
```

For `my_app::math::add`:
- Module page: `my_app/math/index.html`
- Item page: `my_app/math/fn.add.html`

Item pages are prefixed by kind (`fn.`, `struct.`, `class.`, `enum.`,
`trait.`, `type.`, `const.`) to avoid collisions.

### 5.9 Incremental

v1: re-build every time. Docs are small (~1 MB for a medium project).
v2: hash-based invalidation.

---

## 6. Implementation plan

### Files to touch

| Phase | File | Change |
|---|---|---|
| 1 | `crates/riven-core/src/lexer/token.rs` | (no change — `DocComment` already exists) |
| 1 | `crates/riven-core/src/parser/ast.rs` | Add `doc_comments: Vec<String>` to every documentable node |
| 1 | `crates/riven-core/src/parser/mod.rs:455-458`, `:884-887`, `:1112-1115`, `:1187-1190` | Capture instead of discard; attach to next item |
| 1 | `crates/riven-core/src/hir/nodes.rs` | Add `doc_comments` to HIR variants |
| 1 | `crates/riven-core/src/resolve/mod.rs:375`, `:811`, `:821` | Copy doc comments ast → hir |
| 1 | `crates/riven-core/src/resolve/symbols.rs:140-146` | Add `doc_comments` to `Definition` |
| 1 | `crates/riven-core/src/formatter/format_items.rs` | Emit doc comments before items (from AST, not tokens) |
| 2 | `crates/rivendoc/` *new crate* | See §5.2 |
| 2 | `crates/rivendoc/Cargo.toml` | Deps: `pulldown-cmark`, `serde_json`, `riven-core` |
| 3 | `crates/riven-cli/src/cli.rs:25-114` | Add `Doc` subcommand |
| 3 | `crates/riven-cli/src/main.rs` | Wire `Command::Doc` to shell out to `rivendoc` |
| 4 | `crates/riven-ide/src/hover.rs:75` | Render `def.doc_comments` after signature |
| 5 | `install.sh` | Copy `rivendoc` binary; ship assets |
| 5 | `.github/workflows/release.yml:79-103` | Stage `rivendoc` into release tarball |

### Phase breakdown

**Phase 1 — AST capture (2 days).**
The highest-leverage change in this doc. Once it lands, LSP hover
instantly improves (doc 01 §7) and `rivendoc` has something to
extract.

- Day 1: AST + parser changes.
- Day 2: HIR + resolver + symbol table + formatter.

**Phase 2 — `rivendoc` core (5 days).**
- Day 1: crate skeleton, collection pass.
- Day 2: HTML templates (module, function, class, struct).
- Day 3: Markdown + `rvn` highlighting.
- Day 4: cross-ref resolution + search index.
- Day 5: snapshot tests (see §10).

**Phase 3 — CLI integration (1 day).**
`riven doc` wrapping `rivendoc`.

**Phase 4 — LSP hover integration (0.5 day).**
Render doc in hover.

**Phase 5 — Distribution (0.5 day).**
Ship `rivendoc` in release artifacts.

Total: ~9 engineer-days.

---

## 7. Interactions with other tier-3 items

- **Doc 01 (LSP).** The AST change in Phase 1 directly unblocks LSP
  hover enrichment. Ship together if possible.
- **Doc 03 (test framework).** `rivendoc` can link to tests that cover
  each item. Nice-to-have.
- **Doc 05 (bench).** Similar — link bench results to items.
- **Doc 06 (incremental).** Docs generation should run against the
  same query layer for incremental rebuilds. v1 re-builds everything.
- **Doc 08 (property testing).** Property tests could be rendered
  as examples. v2.

### Tier-1 dependencies

- **Tier-1 doc 01 (stdlib).** Once stdlib lands, `rivendoc` becomes
  how users discover it. Stdlib must ship with doc comments on every
  exported item. Coordinate with doc 01 authors.
- **Tier-1 doc 05 (derive).** Deriving `Debug` etc. — the derive
  annotations should surface in docs ("Implements: Debug, Clone").

---

## 8. Phasing

| Phase | Scope | Days | Ships to users? |
|---|---|---|---|
| 1 | AST capture | 2 | No (internal only) |
| 2 | `rivendoc` core | 5 | Yes |
| 3 | CLI + LSP hover | 1.5 | Yes |
| 4 | Distribution | 0.5 | Yes |
| 5 (v2) | Doctests | — | — |
| 6 (v2) | Themes | — | — |

---

## 9. Open questions & risks

1. **OQ-1 — Block doc comment syntax.**
   Line-only `##` for v1 (recommended). Add `#| ... |#` in v2 if
   demand surfaces.
2. **OQ-2 — Doc comment placement precision.**
   What if there's a blank line between `##` and the item? Reject
   (doc comments must be immediately adjacent — matches Rust and avoids
   ambiguity with floating comments).
3. **OQ-3 — Markdown flavor.**
   CommonMark or GFM? Recommend CommonMark + tables only. Matches
   rustdoc's early behavior.
4. **OQ-4 — Public-only default.**
   Document only `Visibility::Public`. `--document-private` is v2.
5. **OQ-5 — Multi-line signatures.**
   `def foo(a: Int, b: Int, c: Int) -> Int` that wraps across 3 lines
   in source: how does it render? Proposal: reuse
   `riven_core::formatter` to pretty-print the signature; always render
   the formatted form.
6. **OQ-6 — `riven.toml` metadata.**
   Project name, version, description — should flow from `riven.toml`
   into the HTML `<title>` and index page. Requires `rivendoc` to
   read `riven.toml`.
7. **OQ-7 — Versioned docs.**
   Publishing multiple versions of a project's docs is a package-registry
   concern. Out of scope v1.
8. **R1 — Doc comments on anonymous impl blocks.**
   Low value. Skip; no doc page for impls in v1.
9. **R2 — Cross-refs to stdlib items.**
   Works if `rivendoc` can resolve `std.*` paths. Requires loading
   stdlib's symbol table. v1 punt: document `[std.Vec.push]` as plain
   text if not found in the current project.
10. **R3 — `pulldown-cmark` dependency weight.**
    ~200KB compiled. Acceptable for a tooling binary. Not shipped to
    users' programs.
11. **OQ-8 — Search-index granularity.**
    Name-only, or also search doc-comment bodies? Recommend name +
    signature + first line of doc comment for v1.
12. **OQ-9 — Linking to source.**
    Each item page should have a "[src]" link to the source file +
    line (GitHub-style). Requires project URL config; simpler: link
    to `file:///absolute/path:line` for local use. Decide for v1.

---

## 10. Test matrix

Snapshot tests (`insta`-style) are the right fit — a set of tiny
fixture projects + their expected HTML output.

| Fixture | Assertion |
|---|---|
| `tests/fixtures/empty/` | Empty project produces index.html with no items |
| `tests/fixtures/one_fn/` | `def foo` with `## doc` renders one page; doc visible |
| `tests/fixtures/struct_with_methods/` | Struct page lists methods; method pages link back |
| `tests/fixtures/enum_with_variants/` | Enum page shows variants |
| `tests/fixtures/xref_internal/` | `[foo]` resolves to `fn.foo.html` |
| `tests/fixtures/xref_missing/` | Unresolved `[bar]` stays as plain text + warning |
| `tests/fixtures/markdown/` | Bold / lists / code blocks render correctly |
| `tests/fixtures/rvn_highlight/` | `rvn` code block has span classes |
| `tests/fixtures/private_hidden/` | Private `def` does not appear in output |
| `tests/fixtures/search_index/` | `search-index.json` lists every public item |
| `tests/fixtures/nested_modules/` | `a.b.c` produces `a/b/c/` paths correctly |

Plus two meta tests:
- Every output page is valid HTML5 (pass through `html5ever`).
- Every internal link in the output points to a file that exists.
