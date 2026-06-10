# ADR: Ruby-style `alias` keyword for Ruxen (`alias new_name old_name`)

Status: Accepted (2026-06-10)
Branch: `feat/drop-elaboration`
Builds on: `docs/decisions/ruby-block-semantics.md` (commit `8a783f9`) — same
keyword-feature house style; reuses its resolve/typeck/MIR seams.

## Context

The Ruxen stdlib repeatedly declares the **same function twice** to give a
method two names — once for each spelling. Examples in `library/std/**`:

- `def length -> Int; self.size; end` next to `def size`.
- `def count` delegating to `size`.
- a `?`-name and a longhand spelling of the same predicate.

Every such pair is a second method body the compiler lowers and codegen emits —
extra symbols, extra call frames, and a maintenance hazard (the two bodies drift).
Ruby solves this with `alias new_name old_name`: a second NAME bound to the SAME
method, not a second method. The user asked for exactly this: "we should allow
the alias keyword. currently most of the stdlib declares same kind of function
twice for alias right now."

### Representation facts (verified in-tree)

- **Method calls mangle by NAME STRING, not DefId.** `resolve/exprs.rs` sets
  `HirExprKind::MethodCall { method_name: method.clone() }`; MIR's
  `select_method_symbol_name` (`mir/lower/mod.rs:439`) emits the callee symbol as
  `format!("{Class}_{method_name}")` by scanning `DefKind::Method` whose
  `def.name == method_name`. So a method named `member?` aliasing `include?` would
  mangle to `Set_member?` — a symbol with no body → link error. The alias name
  must therefore be **rewritten to the canonical name** before MIR mangles, OR the
  symbol-selection layer must be taught the synonym.
- **Free-fn calls resolve a callee DefId by name** (`bind_callable_name`,
  `scopes.insert(name → DefId)`), then mangle by the resolved def's `name`. So a
  free-fn alias is a pure value-scope rebind: `scopes.insert(alias → target_DefId)`
  with NO new def — both names resolve to one DefId, one body.
- **Precedent — FFI aliasing already exists.** `FfiFuncDecl` carries both `name`
  (linked C symbol) and `ruxen_name` (call-site spelling); `lower/mod.rs`'s
  `lookup_ffi_alias` / `ffi_alias_map` rewrites the call-site name to the C symbol
  at lower time. The method-alias mechanism here is the same shape applied to
  Ruxen-side methods.

## Decision

### D1 — Syntax: `alias new_name old_name` (Ruby space form only)

`alias new_name old_name` — two bare names separated by whitespace, Ruby keyword
style. It is an **item** valid in:

- `class` / `struct` / `enum` / `mixin` bodies and `extension` impl bodies
  (a **method** alias, scoped to the enclosing type), and
- **top level** and `module` bodies (a **free-function** alias).

The comma form `alias new_name, old_name` is **rejected** — Ruby's `alias` takes
no comma (only `alias_method :new, :old` does, which is a method call, not the
keyword). A comma after the first name is a clean parse error. This keeps one
spelling and avoids a second grammar to format.

`alias` is a **contextual keyword**, not a reserved word: it only starts an alias
item when it appears in item position (start of a body line / top-level line)
followed by an identifier. Everywhere else `alias` lexes as an ordinary
identifier, so existing code/methods/variables named `alias` keep working. This
mirrors how `var` / `some` / `any` / `match` are handled (parser/CLAUDE.md
"Contextual keywords"). No entry is added to the `KEYWORDS` table — adding one
would reserve the word globally and break any `.alias` field or `alias` local.

### D2 — Semantics: pure resolver synonym, NOT delegation

Both names resolve to **ONE** function body — zero duplicated codegen, zero extra
call frame. There is no synthesized `def new(...) = old(...)` thunk.

- **Free-fn alias** → value-scope rebind: `alias foo bar` binds `foo` in the
  enclosing value scope to `bar`'s existing DefId. A call to `foo(...)` resolves
  the same DefId as `bar(...)` and mangles to the same symbol.
- **Method alias** → a per-type **synonym map** `{alias_name → canonical_name}`
  recorded on the resolver and threaded to typeck (`type_methods` lookup resolves
  the alias name to the canonical signature) and to MIR (`select_method_symbol_name`
  falls through the synonym map so `set.member?(x)` emits `Set_include?`). No
  `DefKind::Method` is created for the alias; `ClassInfo.methods` is unchanged.

#### Divergence from Ruby (documented)

Ruby's `alias` **copies the method at alias time** — a later redefinition of
`old_name` does NOT change `new_name`, because Ruby can redefine methods at
runtime. Ruxen is statically compiled with no runtime redefinition, so this
subtlety is **moot**: `alias` is a pure compile-time synonym that always resolves
to the single body the target name denotes at compile time. We record this as an
explicit divergence: **Ruxen `alias` is a synonym, not a snapshot copy.**

### D3 — Overload rule: target must be unambiguous (single arity)

Aliasing binds the alias name to the target name. If the target is an
**overload set** (same name, multiple arities), the alias would have to bind ALL
arities to be sound — but a method-name synonym is arity-agnostic by
construction, so `alias a b` where `b` is overloaded resolves to whichever arity
the call site selects. We therefore **accept** aliasing an overloaded target: the
alias binds the NAME, and overload selection at the call site picks the arity
exactly as it would for the canonical name. This is the simplest sound rule (the
synonym is applied before overload resolution, so the existing arity machinery
runs unchanged). No diagnostic for overloaded targets.

### D4 — Alias-of-alias: resolves transitively to the root, no chains stored

`alias c b` followed by `alias b a` (both targeting eventually-`a`) resolves `c`
to `a`. We **flatten at registration time**: when registering `alias new old`, if
`old` is itself an alias, we look through to its already-recorded canonical target
and store `{new → root}`. This guarantees the synonym map is always one hop deep
(no transitive walk at lookup, no cycle risk). Registration order within a body
is top-to-bottom; an alias whose target alias appears LATER in the same body
resolves in a second settling pass (the body's aliases are collected first, then
flattened). A cycle (`alias a b` + `alias b a`) is detected at flatten time and
rejected with **E1121** (see D6).

### D5 — Diagnostics

- **Unknown target** — `alias foo nonexistent` where no method/free-fn named
  `nonexistent` is visible in the alias's scope → **E1120** ("alias target not
  found"). For a method alias the target is sought among the enclosing type's
  methods (incl. included-mixin methods, D7); for a free-fn alias, among visible
  callables.
- **Collision** — `alias foo foo` (new == old), or an alias whose new name
  already names a real method/free-fn in the same scope → **E1122** ("alias name
  collides with an existing definition"). Self-alias (`alias x x`) is the
  degenerate collision case and is also E1122.
- **Alias cycle** — a flatten-time cycle (D4) → **E1121** ("alias forms a cycle").

All three are registered in `diagnostics::codes::REGISTRY` with
`docs/errors/E112{0,1,2}.md`.

### D6 — Operator aliases: STAGED with a clean diagnostic (Tier 2)

`alias << push` / `alias [] get` (operator-symbol new names, or operator targets)
route through the operator-method machinery (`a << b` → `a.<<(b)`, `a[i]` →
`a.[](i)`; the desugar is post-typeck in `mir/lower/expr/binops.rs` +
`typeck/infer/ops.rs`). Wiring an operator NAME as an alias requires the synonym
map to participate in the operator-desugar symbol selection, which lives in a
different lowering path than ordinary method-name mangling. Rather than half-wire
it, Tier 1 **rejects an operator-spelled alias name or target** with a clear,
actionable diagnostic **E1123** ("operator aliases are not yet supported")
pointing at the staged follow-up (filed in `docs/TASKS.md`). Plain `?`/`!`
method names (`alias member? include?`, `alias add! push`) are NOT operators —
they are ordinary identifiers the lexer absorbs into the name — and ARE supported
in Tier 1.

### D7 — Alias through `include` (mixin methods)

An alias may target a method the type **defines itself**, including one that
satisfies a mixin requirement (`mixin Shape { def area }`, the class defines
`area` and `alias surface area`). Both names resolve to the one body and the
mixin contract is still satisfied — the alias is a pure synonym, not a new
method. Implementors see both names. Pinned (REVIEWER pin e).

**Tier-1 boundary (staged):** aliasing a method the type gets ONLY from a mixin
**default body** (not redefined in the type) is staged for Tier 2. A mixin
default's signature lives in the mixin's signature table
(`typeck::trait_method_sigs`), not the type's own method table (`type_methods`),
so the typeck-side synonym registration cannot yet bind it. Filed in
`docs/TASKS.md`. (The common case — aliasing a type's OWN method — works; this
boundary only affects aliasing an inherited default verbatim.)

### D8 — Generic bounds + Q17 monomorphization

A generic free fn `def render_all[T: Paintable](xs)` aliased as
`alias paint_all render_all`, and a generic-bound call made via the ALIAS name,
monomorphize **identically** — because the alias is resolved to the canonical
name/DefId *before* the Q17 worklist (`collect_generic_fn_instances`) ever runs.
The monomorphizer only ever sees the canonical name. Pinned (REVIEWER pin f).

### D9 — Visibility

An alias **inherits the target's visibility** — it is a second name for the same
def, so it cannot be more public than what it points at. A subsequent
`private :alias_name` (the Ruby name-list marker, parser/methods.rs
`parse_visibility_name_list`) re-marks the alias like any other name. The alias
item itself carries no visibility token.

### D10 — `ruxen fmt` round-trips the alias item byte-stably

The formatter emits `alias new_name old_name` on its own line, preserving order
and the single-space form. No normalization (no comma insertion, no reordering).
Pinned: `format(format(x)) == format(x)` and the literal `alias a b` survives
(REVIEWER pin i). Avoids a fmt-destructiveness incident (cf. Q23/Q30/Q34).

## Out of scope

**Staged (Tier 2), filed in `docs/TASKS.md`:**
- Operator-spelled aliases (`alias << push`, `alias [] get`) — D6, E1123.
- `alias_method`-style runtime-ish reflective form (no such surface in Ruxen).

**Rejected:**
- The comma form `alias new, old` — D1.
- Delegation thunks — defeats the zero-codegen purpose (D2).
- `alias` as a hard reserved keyword — would break identifiers named `alias`
  (D1, contextual-keyword decision).

## Consequences

- The stdlib sweep (Part 2) replaces true duplicate-body synonyms with `alias`
  lines, removing N method bodies (see CHANGELOG / report). Cases where two `.rx`
  decls differ in RETURN TYPE (e.g. array `get` `&T` vs `get_mut` `&var T`) are
  NOT synonyms and are deliberately left alone — a pure synonym cannot express a
  differing signature.
- New error codes E1120–E1123 enter the registry (range E1120+ continues the
  Ruby-naming/keyword-feature block after E1119).
- The synonym map is a small per-type / global `HashMap<String,String>` threaded
  resolve → typeck → MIR; it adds no new HIR/MIR node and does not touch codegen.
