# Tier 5 — Deprecation & Stability Directives

Status: draft
Depends on: tier1 B2 (directive-arg plumbing). Tier5_02 (editions) and
tier5_04/05 (diagnostics) consume this.
Blocks: a stable stdlib, a migrator that can deprecate old APIs, any
library author who wants to evolve published APIs without a major
version bump.

---

## 1. Summary & motivation

Libraries outlive their authors' good taste. A v1 API reveals a flaw;
the author wants to ship a replacement without breaking v1 callers;
users want to know "this is deprecated, here's what to use instead."

Rust solves this with three attributes (internal Rust reference for
prior art):

- `#[deprecated(since = "1.42", note = "use `Foo::new` instead")]`
- `#[unstable(feature = "try_trait", issue = "42")]`
- `#[stable(since = "1.0.0", feature = "try_trait")]`

and rustc emits warnings / errors accordingly. The stdlib itself uses
these to graduate APIs from nightly to stable. Riven expresses the
same concepts via **in-body directives** — Ruby-flavored statements
inside the item body, not prefix annotations.

Riven has **none** of this. Zero recognition of `deprecated`, `stable`,
or `unstable`. The stdlib (tier 1) cannot evolve without breakage. The
edition mechanism (tier 5 doc 02) cannot gate unstable features without
it. The LSP (tier 3) cannot show "this function is deprecated" hovers.

This document specifies:

- In-body directive syntax for deprecation / stability.
- Compiler behaviour (warn at use sites; gate unstable; record
  stability in metadata).
- Stability levels and their meanings.
- Allow-list / deny-list behaviour (in-body `allow deprecated`).

---

## 2. Current state

### 2.1 What directive syntax works today

`parser/ast.rs:787-793` (internal Rust — name retained for compiler
contributors):

```rust
pub struct Attribute {
    pub name: String,
    pub args: Vec<String>,     // ← stringly-typed!
    pub span: Span,
}
```

Parser (`parser/mod.rs:1572-1610`, `parse_attributes`) currently accepts
the old `@[name(arg1, arg2, ...)]` prefix form. Dispatch logic at
`parser/mod.rs:473-511` handles only the legacy `link`/`repr` cases;
anything else on the top-level path errors out. The Ruby-flavor
migration retires the `@[...]` syntax — these forms are now in-body
directives (`lib "x"` options, `layout c`, etc.), and the parser is
being reworked accordingly. The internal Rust type name `Attribute`
remains for compiler-internal use.

### 2.2 What's missing

- No `deprecated`, `stable`, or `unstable` in-body directive handler.
- No key-value directive args: `since: "0.3"` would fail parsing today.
- No warning mechanism for "use of deprecated item."
- No stability tracking on symbol-table entries
  (`resolve/symbols.rs:48-75` has no `stability:` field).
- No in-body `allow deprecated` / `warn ...` lint-level controls.

### 2.3 Warnings infrastructure (partial)

`DiagnosticLevel::Warning` exists (`diagnostics/mod.rs:7`) and is
threaded through the pipeline. `Diagnostic::warning` constructor exists
(`:48-55`). Borrow-check does not emit any warnings today; typeck emits
a handful. No lint-level system that says "this warning can be
suppressed by directive X."

---

## 3. Goals & non-goals

### 3.1 Goals

- In-body `deprecated since: "0.3", note: "use `Foo.new`"` directive
  parses, round-trips through HIR and metadata, and produces a warning
  diagnostic on use.
- `unstable feature: "...", issue: "..."` directive parses; use
  requires opt-in (tier5_02).
- `stable since: "0.2", feature: "..."` directive parses and records.
- In-body `allow deprecated` / `warn deprecated` / `deny deprecated`
  on a containing item suppresses or promotes the warning for that
  scope.
- Apply to all item kinds: `def` (fn), `class`, `struct`, `enum`,
  `mixin`, `const`, `type` alias, `use` re-export, enum variant, struct
  field, method.
- LSP hovers show stability and deprecation.
- Metadata (`.rivenmeta`) round-trips stability.

### 3.2 Non-goals

- Per-edition stability overrides (that's tier5_02).
- Time-based deprecation sunset ("turns into error in 2027"). The
  edition is the escalation mechanism.
- Platform-gated stability (`stable target: "linux"`). Not needed
  for v1; future extension.
- Deprecation attached to mixins-only (not includes). Deprecating an
  `include` without deprecating the mixin is a niche case; skip for
  v1.
- `must_use` directive — separate lint, separate doc (future work).

---

## 4. Surface

### 4.1 Directive grammar (to be specified in tier5_01 §03-grammar/06)

A directive is an in-body statement of the form `name arg-list?`,
appearing at the top of an item's body (above any method or field).
Several directives may stack on the same item, each on its own line.

```ebnf
directive       ::= ident arg-list?
arg-list        ::= directive-arg ( ',' directive-arg )* ','?
directive-arg   ::= literal
                  | ident ':' literal
                  | ident '(' arg-list ')'        # nested
literal         ::= string-literal
                  | int-literal
                  | bool-literal
ident           ::= <identifier>
```

The internal HIR carrier (Rust name preserved for compiler
contributors) holds the parsed args as a typed list:

```rust
pub struct Attribute {
    pub name: String,
    pub args: Vec<AttrArg>,
    pub span: Span,
}

pub enum AttrArg {
    Lit(AttrLit),                   // `"0.3"` or `42` or `true`
    KeyValue(String, AttrLit),      // `since: "0.3"`
    Nested(String, Vec<AttrArg>),   // `feature(name: "x")`
}

pub enum AttrLit {
    String(String),
    Int(i64),
    Bool(bool),
}
```

### 4.2 Deprecation directive

```riven
def from_utf8_lossy_impl(bytes: &Array[UInt8]) -> String
  deprecated since: "0.3",
             note: "use `String.from_utf8_lossy` instead"

  ...
end
```

Arguments:

| Key | Type | Required? | Meaning |
|-----|------|-----------|---------|
| `since` | string | yes | semver version when deprecated |
| `note`  | string | optional but strongly recommended | free-form migration hint |

Omission: a bare `deprecated` directive (no `since`, no note) is
valid. The compiler emits a vaguer warning.

Semantics:

- Use site of the deprecated item → emit `W2001` warning by default.
  (Warnings get `W` prefix; see tier5_04 §4.1.)
- Warning text: `` warning[W2001]: use of deprecated `X`: <note> ``.
- Warning appears at the USE site, not the def site.
- Expression-level uses counted: `foo()` call, `Foo` in a type
  position, `Foo.BAR` variant, `struct.field` access.
- Re-export is NOT a use: `use old_module.OldFn` does not warn.
  (Matches Rust.)

### 4.3 Unstable directive

```riven
mixin Try
  unstable feature: "try_mixin", issue: "42"

  def into_result(self) -> Result[Self.Ok, Self.Err]
end
```

Arguments:

| Key | Type | Required? | Meaning |
|-----|------|-----------|---------|
| `feature` | string | yes | feature-gate name |
| `issue` | string or int | optional | tracking issue |
| `reason` | string | optional | one-liner rationale |

Semantics:

- Use of the item is an **error** (`E2002`) unless the consuming crate
  opts in via manifest `[features]` + `unstable = […]` (tier5_02 §4.5).
- Nightly-or-override channel bypasses the error.
- Error text: `` error[E2002]: use of unstable feature `try_mixin`;
  add `features = ["try_mixin"]` to Riven.toml ``.
- Recursive: if a stable function calls an unstable function, the
  stable function is implicitly-unstable (emits an error on use unless
  opted in). This is the same rule as Rust.

### 4.4 Stable directive

```riven
def String.trim_ascii(self) -> &str
  stable since: "0.2.0", feature: "string_ext"

  ...
end
```

Arguments:

| Key | Type | Required? | Meaning |
|-----|------|-----------|---------|
| `since` | string | yes | compiler version when it became stable |
| `feature` | string | yes | the same name as the former `unstable` |

Semantics:

- Marks an item as "promoted from unstable." Consumers don't need a
  feature gate.
- At most one of `stable` and `unstable` directives on the same item.
- Stable items may be deprecated: a `stable ...` directive and a
  `deprecated ...` directive in the same body coexist.
- Items without any of the three directives default to "stable since
  the containing crate's version." (Rust's implicit rule; we inherit
  because making every fn declare `stable` is unhelpful.)

### 4.5 Lint-level controls

A small family of in-body directives on any *enclosing* item that
change the level of specific lints inside its body:

```riven
def use_old_api(s: String) -> String
  allow deprecated

  old_api(s)        # no warning emitted
end
```

| Directive | Effect |
|-----------|--------|
| `allow name` | lint `name` emits nothing inside this scope |
| `warn name`  | lint `name` emits at warning level |
| `deny name`  | lint `name` emits at error level |
| `forbid name` | like deny, but cannot be overridden by inner allow |

Supported lint names (v1):
- `deprecated`
- `unused` (future)
- `dead_code` (future)

### 4.6 Scope of attachment

Directives live in the body of the item they modify (Ruby-style), at
the top of the body above method/field declarations. Supported on:

- `def` (top-level + inside class)
- `class`, `struct`, `enum`, `mixin`
- `extension` blocks
- `const`
- `type` alias, `newtype`
- enum variants (inside the enum body)
- struct fields (inside the struct body)
- methods (inside `class` or `extension` body)

Not supported in v1:
- Expressions (an `allow ...` directive on a bare expression)
- Statements (an `allow ...` directive on `let x = ...`)
- Standalone blocks
- Modules (but on items *within* a module, yes)

(Rust allows annotation in these positions; we defer.)

---

## 5. Architecture / design

### 5.1 AST changes

`parser/ast.rs:787-793` — widen the `Attribute` carrier (internal Rust
name retained; user-facing concept is "in-body directive"):

```rust
pub struct Attribute {
    pub name: String,
    pub args: Vec<AttrArg>,
    pub span: Span,
}

pub enum AttrArg {
    Lit(AttrLit),
    KeyValue(String, AttrLit),
    Nested(String, Vec<AttrArg>),
}
pub enum AttrLit {
    String(String),
    Int(i64),
    Bool(bool),
}
```

This breaks tier-1 B2's "hack" where `repr(C)` data was stuffed into
`derive_traits: Vec<String>` — which tier-1 already calls out needs
untangling. Under the Ruby-flavor migration, `repr(C)` is itself
replaced by the in-body `layout c` directive (tier5_01 §3.5); the
internal `Attribute` carrier continues to model both for the
transitional code paths.

### 5.2 HIR changes

Each item kind (`HirFuncDef`, `HirStructDef`, `HirClassDef`,
`HirEnumDef`, `HirMixinDef`) grows a field:

```rust
pub stability: Stability,
```

Where `Stability` is:

```rust
pub enum Stability {
    Stable { since: Option<Semver>, feature: Option<String> },
    Unstable { feature: String, issue: Option<String>, reason: Option<String> },
    Deprecated { since: Option<Semver>, note: Option<String>,
                 underlying: Box<Stability> },  // stable + deprecated is common
}

impl Stability {
    pub const fn default() -> Self { ... }  // "implicitly stable"
}
```

### 5.3 Symbol-table changes

`resolve/symbols.rs`:

```rust
pub struct Definition {
    pub name: String,
    pub kind: DefKind,
    pub span: Span,
    pub stability: Stability,     // NEW
    ...
}
```

Stability is propagated from the HIR item into the definition at
resolve time.

### 5.4 Warning emission pass

Either:
- **(A)** New pass `lints/deprecation.rs` between typeck and borrow
  check, walking every `ExprKind::Call`, `ExprKind::FieldAccess`,
  `ExprKind::Path`, `TypeExpr::Named` and looking up stability in the
  symbol table.
- **(B)** Inline checks inside existing passes (e.g. at every symbol
  lookup in `resolve/mod.rs`).

**Recommendation:** (A). Keeps concerns separate. A single
`lints/deprecation.rs` file is easier to reason about than sprinkled
checks. The lint pass reads the fully-resolved HIR — everything it
needs is available.

Pseudocode sketch:

```rust
pub fn run_deprecation_lint(
    program: &HirProgram,
    symbols: &SymbolTable,
    ctx: &EditionCtx,
    allow_stack: &mut LintLevelStack,
) -> Vec<Diagnostic> {
    let mut diags = vec![];
    walk_hir(program, |node| {
        if let Some(def_id) = referenced_def(node) {
            let stab = symbols.get(def_id).stability;
            match stab {
                Stability::Deprecated { note, since, .. } => {
                    if allow_stack.level_of("deprecated") >= Warn {
                        diags.push(Diagnostic::warning_with_code(
                            format!("use of deprecated `{}`: {}", ..., note),
                            node.span.clone(),
                            "W2001",
                        ));
                    }
                }
                Stability::Unstable { feature, .. } => {
                    if !ctx.features.contains(feature) && !ctx.allow_unstable {
                        diags.push(Diagnostic::error_with_code(
                            format!("use of unstable feature `{}`", feature),
                            node.span.clone(),
                            "E2002",
                        ));
                    }
                }
                _ => {}
            }
        }
    });
    diags
}
```

### 5.5 Lint-level stack

A tiny stack-scoped map keyed by item. Entering an item with an
in-body `allow deprecated` directive pushes a frame; leaving pops it.
The lookup `allow_stack.level_of("deprecated")` returns the
tightest-enclosing level. The **default** level for `deprecated` is
`Warn`; for `unstable` there is no level (it's not a lint, it's an
error).

### 5.6 Metadata round-trip

The canonical-name / metadata system introduced in tier5_02 §5.5 must
also carry `Stability` — otherwise a deprecated stdlib item loses its
deprecation when consumed from another crate. The `.rivenmeta` schema
gets a `stability:` field per exported symbol.

### 5.7 LSP integration

`crates/riven-ide/src/hover.rs` should, on hover of a symbol with
deprecation:

- Prepend `**Deprecated since 0.3** — use `Foo.new`.\n\n---\n\n` to
  the hover markdown.
- Include the stability feature name and issue link if the item
  carries an `unstable` directive.
- Surface the full rendering to VSCode / whichever client via the
  existing hover pipeline.

`riven-ide/src/diagnostics.rs:11-28` already passes `diag.code`
through to LSP; W-prefixed codes work identically.

---

## 6. Implementation plan

### 6.1 Phase 3a — directive-arg plumbing (1 week)

This is the **prerequisite** for both this doc and tier1 B2 (auto-
implicit-include untangling). Doing it once pays for both.

1. Widen `AttrArg` and `AttrLit` in `parser/ast.rs:787`.
2. Update `parse_attributes` (`parser/mod.rs:1572-1610`) to parse
   in-body key-value and nested forms.
3. Update `parser/printer.rs:114-118` and `formatter/format_items.rs`
   to round-trip new arg shapes.
4. Update `parser/mod.rs:473-511` to dispatch in-body `deprecated`,
   `stable`, `unstable` at the same level as `lib`-options and
   `layout`.
5. Fail gracefully on unknown directive with `W2999: unrecognized
   directive \`foo\`; ignored`.

**Exit criterion:** `deprecated since: "0.3", note: "x"` in an item
body parses and appears in `--emit=ast` output.

### 6.2 Phase 3b — stability in HIR and symbols (1 week)

1. `Stability` enum in `hir/nodes.rs` (or a new `hir/stability.rs`).
2. `HirFuncDef.stability`, `HirStructDef.stability`, etc.
3. Pass resolver copies `stability` into `Definition.stability`
   (`resolve/symbols.rs`).
4. Formatter round-trips in-body directives on items.

### 6.3 Phase 3c — deprecation warnings (1 week)

1. `lints/deprecation.rs` pass.
2. Wire after typeck in `rivenc/src/main.rs` and the cache's
   `compile_to_object`.
3. Register `W2001` (deprecated use) in the error-code registry.
4. In-body `allow deprecated` support on enclosing items.

**Exit criterion:** fixture with a `deprecated`-directive function +
caller emits `W2001`; fixture with caller's body opening `allow
deprecated` emits nothing.

### 6.4 Phase 3d — unstable gating (1-2 weeks)

Requires tier5_02 phase 2c (manifest `[features]`) — do after or
alongside.

1. Register `E2002` (unstable feature use).
2. In the deprecation-lint pass, extend to handle `Stability::Unstable`.
3. Read `EditionCtx.features` to decide whether to gate.
4. Fixture: item with `unstable feature: "x"` directive; caller
   without `x` in features → E2002; with `x` → OK.

### 6.5 Phase 3e — metadata round-trip + LSP (1 week)

1. Serialize `Stability` into `.rivenmeta`.
2. Deserialize on import, attach to imported `Definition`.
3. Fixture: cross-crate deprecation warning.
4. `riven-ide/src/hover.rs` prepends deprecation notice.

---

## 7. Interactions with other tiers

- **Tier 5 doc 02 (editions):** in-body `unstable feature: "…"` is
  read alongside manifest `[features]`. Edition-deprecation lints
  reuse the same `deprecated`-directive machinery.
- **Tier 5 doc 04 (error codes):** reserves the ranges:
  - `W2001` — deprecated-use warning.
  - `E2002` — unstable-without-opt-in error.
  - `W2999` — unrecognized directive.
- **Tier 5 doc 05 (suggestions):** a deprecation with a `note` may
  include a machine-applicable rewrite suggestion in the future (e.g.
  `deprecated since: "0.3", note: "use `new`", replace: "new"` for
  common cases). Defer to future work; note here so we don't repaint.
- **Tier 1 doc 05 (auto-implicit-include):** the implicit-include
  pipeline for structural mixins (`Debug`, `Clone`, ...) REQUIRES
  phase 3a — the same directive-arg plumbing carries the optional
  loud `include D1, D2` form. Do 3a first, then tier1-B2 and tier1-05
  can proceed.
- **Tier 1 stdlib:** every stable stdlib item should carry a
  `stable since: "0.2.0", feature: "…"` directive once this lands so
  we have a record of when things entered the public surface.
- **Tier 3 LSP:** hovers + completions consume `Stability` from the
  symbol table.

---

## 8. Phasing

Summary (details in §6):

| Phase | Work | Dep | Gate |
|-------|------|-----|------|
| 3a    | Directive-arg plumbing (`AttrArg`, parser, printer, formatter) | — | Tier1 B2, all Tier-5 |
| 3b    | `Stability` in HIR + symbols, propagation | 3a | — |
| 3c    | Deprecation warnings + in-body `allow deprecated` | 3b | Stdlib evolution |
| 3d    | Unstable gating | 3b + tier5_02 §2c | Unstable stdlib surfaces |
| 3e    | Metadata round-trip + LSP hover | 3c + 3d | Published libraries |

Total: ~5-7 weeks.

---

## 9. Open questions & risks

### OQ-1. Directive-arg grammar: strictly key-value or positional allowed?

Rust's `#[deprecated("message")]` shorthand is confusing — it maps to
`note = "message"`. **Recommended:** Riven is **strictly keyword**. The
shorthand is rejected with a good error: `E0921: expected key: value
form, e.g. deprecated note: "..."`. Enforces one way to do it.

### OQ-2. Directive naming style — `deprecated` or `deprecate` verb-
    forms? Lowercase or CamelCase?

**Recommended:** lowercase identifiers matching Ruby/Rust idiom
(`deprecated`, `stable`, `unstable`, `allow`, `warn`, `deny`,
`forbid`). Riven's P2 (tutorial-aligned principle: "Ruby-flavored
where Ruby has a convention") leans this way.

### OQ-3. Should `stable ...` be required on public items?

Rust doesn't require it (implicit default applies). **Recommended:**
Riven also doesn't require it — it would be noise on every stdlib
function. Document the implicit default in tier5_01
§03-grammar/06-directives.md.

### OQ-4. Stdlib: how many items should carry `unstable` initially?

Depends on what tier-1 phases look stable enough. **Recommended:** at
launch, no stdlib items are unstable — we ship what we ship. First
uses of the `unstable` directive arrive when a new feature is under
iteration (e.g. an experimental async reactor API).

### OQ-5. Does `deprecated` on a mixin deprecate all its includes?

**Recommended:** No. Deprecating a mixin emits a warning at every use
of the mixin bound, every `include MixinFoo` (the `include` is "using"
the mixin), and every `any MixinFoo`. The classes that `include` the
mixin do not themselves become deprecated. Matches Rust.

### OQ-6. In-body `allow deprecated` on a whole crate?

**Recommended:** support it on the top-level `use` re-export or on any
item at top-level. A crate-root form (an inner-style directive
attached to the package root) is **not** supported in v1 — adds
grammar complexity. File-level `allow ...` via a wrapping `module`
declaration is the workaround.

### OQ-7. Risk: silent cycles — a deprecated item calls another
      deprecated item.

Mitigation: the lint pass tracks "reported spans" and deduplicates. A
deprecated method's body containing a call to a deprecated helper
only fires once, at the caller site in user code.

### OQ-8. Risk: a compiler version mismatch with `stable since: ".X"`

If an old compiler sees a future version in `since`, the item should
still be usable (compiler version check is not a gate; the `since` is
purely informational). **Recommended:** yes. The manifest's `riven =
">=X"` is where version enforcement lives.

### OQ-9. Nested directives: how deep?

**Recommended:** one level of nesting max. `deprecated replace:
list(from: "a", to: "b")` is already too much. Rust allows arbitrary
nesting in its attribute grammar; we don't need it for any directive
specified here and can lift the restriction later if a real use
arrives.

### OQ-10. Risk: directives on macros and macro-generated items get
        lost.

Macros (tier1_05) are future work; when they land, each expansion must
carry the outer directives through to the expanded items. The macro
doc already addresses directive forwarding at a minimum level; detail
deferred.

---

## 10. Acceptance criteria

- [ ] `parser/ast.rs:Attribute` uses `Vec<AttrArg>` with key-value and
      nested support.
- [ ] `parser/mod.rs:473-511` dispatches in-body `deprecated`,
      `stable`, `unstable` at the same level as `lib`-options and
      `layout`.
- [ ] `Stability` enum exists and is a field on every `Hir*Def` item.
- [ ] `Definition.stability` is populated by the resolver.
- [ ] `lints/deprecation.rs` emits `W2001` on use of a
      `deprecated`-directive item.
- [ ] `lints/deprecation.rs` emits `E2002` on use of an
      `unstable feature: "x"` item without `features = ["x"]`
      in the manifest.
- [ ] In-body `allow deprecated` on an enclosing item suppresses
      `W2001` inside its body.
- [ ] In-body `deny deprecated` promotes `W2001` to an error.
- [ ] `.rivenmeta` round-trips `Stability` for exported symbols.
- [ ] LSP hover prepends a "Deprecated" callout when hovering a
      deprecated symbol.
- [ ] Fixture: deprecated item used in-crate → warning with note.
- [ ] Fixture: deprecated item used across crates → warning.
- [ ] Fixture: unstable item used without opt-in → error E2002.
- [ ] Fixture: unstable item used with `features = ["x"]` → OK.
