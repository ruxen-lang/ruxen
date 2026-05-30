# Ruxen Surface-Syntax Migration — Agent Prompt

Paste this entire file as the first message to an AI agent. The agent's
job is to bring every doc, fixture, test, and source file into line
with the canonical surface-syntax spec.

---

## Your role

You are a migration engineer with full access to the repo. The
canonical spec at `docs/specs/syntax/ruby-naming.spec.md` was
rewritten on 2026-05-16 and is now the single source of truth for
Ruxen surface syntax. Every other file (tutorial, stdlib spec,
requirements doc, fixture, example, README, CHANGELOG, source
comment, error message) must conform. Your job is to find and
rewrite the drift.

Read the canonical spec end-to-end **first**. Pay special attention
to:

- §3.4 (mixin diamond resolution + coercion table)
- §3.6 (implicit-include set, with the canonical mixin names)
- §3.9 (path separator is `.`, never `::`)
- §3.10 (nil priority rule)
- §3.12 (receiver modes — `def`, `def var`, `def consume`, `def self.`)
- §3.14 (reserved keywords; `var` plays four positional roles)
- §3.15–§3.22 (comments, strings, closures, tuples, newtypes,
  numeric coercion, for-loop desugaring, struct/enum literals)
- **§10a (migration mapping table — this is your find-and-replace
  source)**

When the spec and any other doc disagree, the spec wins. Do not
edit the spec itself; it is the destination, not a worksite.

---

## Mechanical replacements

These are pure find-and-replace. Apply across every file in scope
(see "Where to sweep" below). The OLD column corresponds to the
LEFT column of §10a in the spec; consult §10a if you need a
disambiguation note.

### Receiver / reference / pointer mutability

| Old token | New token |
|---|---|
| `def mut foo` (method declaration) | `def var foo` |
| `&mut T` (in any type position) | `&var T` |
| `*mut T` (raw pointer) | `*var T` |
| `*mut Void` | `*var Void` |
| `mut self` / `&mut self` (in tutorial prose) | `var self` / "writing method" |
| `FnMut(...)` (closure type name) | `FnVar(...)` |
| `iter_mut()` (method name on collections) | `iter_var()` |
| `deref_mut()` (method name on guards/refs) | `deref_var()` |
| `let mut x = ...` | `var x = ...` |
| `mut` keyword anywhere else | DELETE — `var` covers every "writable" position |

### Mixin renames (built-in mixins only)

| Old name | New name |
|---|---|
| `Hash` (mixin) | `Hashable` |
| `Displayable` | `Display` |
| `Comparable` | `Ord` (full order) or `PartialOrd` (partial), pick by context |
| `Iterable` | `Iterator` (one mixin covers both jobs) |

**Note.** The collection type `Hash[K, V]` (alias) is retired; use
`Map[K, V]`. The mixin `Hashable` is unrelated to the collection;
both exist with the new names. See §10a.

### Collection / smart-pointer type renames

| Old name | New name |
|---|---|
| `Vec[T]` | `Array[T]` |
| `HashMap[K, V]` | `Map[K, V]` |
| `HashSet[T]` | `Set[T]` |
| `Hash[K, V]` (as collection alias) | `Map[K, V]` |
| `Rc[T]` | `Shared[T]` |
| `Arc[T]` | `SharedSync[T]` |

### Trait / impl vocabulary

| Old form | New form |
|---|---|
| `trait T ... end` | `mixin T ... end` |
| `impl T for U ... end` (block) | `include T` directive inside `U`'s body; methods scattered |
| `impl U ... end` (inherent block) | move methods into `U`'s body directly |
| `impl[T: B] C[T] ... end` | `extension C[T] where T: B ... end` |
| `impl Drop ... end` inside class | `include Drop` + `def var drop` in class body |
| `&impl T` (param / return type) | `&some T` |
| `&dyn T` (param / return type) | `&any T` |
| `Box[dyn T]` | `Box[any T]` |
| `'a` lifetime sigil | bare lowercase identifier in `[...]`: `[a]`, `&a T`, `&a var T` |

### Visibility / attribute / directive cleanup

| Old form | New form |
|---|---|
| `pub` prefix on item | DELETE (public is default); use `private` / `protected` section markers |
| `@[derive(D1, D2)]` prefix attribute | DELETE — structural mixins are implicitly included (§3.6); loud form is `include D1, D2` |
| `derive D1, D2` in-body | `include D1, D2` if loud form is wanted; else DELETE |
| `@[repr(C)]` | `layout c` at top of type body |
| `@[repr(packed)]` | `layout packed` |
| `@[repr(transparent)]` | `layout transparent` |
| `@[link(name = "x")]` | `lib "x", ...` options |
| `@[inline]` | `inline def f` modifier; or standalone `inline :f` directive |
| `@[test]` / `@[ignore]` / `@[should_panic]` | in-body `test` / `ignore` / `should_panic` directives |
| `@[bench]` | in-body `bench` directive |
| `@[deprecated]` / `@[stable]` / `@[unstable]` | in-body `deprecated` / `stable` / `unstable` directives |
| `@[no_std]` | package-level `no_std` directive |
| `@[opt_out_send]` / `@[unsafe_impl_send]` | body-level `opt_out_send` / `unsafe_impl_send` (or `exclude Send` / `unsafe include Send` per §3.6) |
| any other `@[...]` prefix attribute | retired; convert to the in-body directive form |

### Literals / paths / FFI

| Old form | New form |
|---|---|
| `None` (constructor for `Option`) | `nil` |
| `null` (FFI null pointer literal) | `nil` (same token; context disambiguates per §3.10) |
| `::` path separator (anywhere) | `.` (`std.io`, `Color.Red`, `package.utils`, `T.method`) |
| `extern "C" ... end` | `lib "c" ... end` (or `lib "<linkname>" ... end`) |
| `crate` (in path or keyword) | `package` |
| `[…]` macro form | bare `[…]` literal — produces `Array[T]` |
| `{…}` macro form | bare `{ k => v, … }` literal — produces `Map[K, V]` |
| `set!{…}` macro | `Set.from_iter([…])` — `{…}` is reserved for `Map` |

### Stdlib API spellings

| Old form | New form |
|---|---|
| `File.read_string(path)` | `fs.read_to_string(path)` |
| `String.new(s)` for converting from `&str` | `String.from(s)` — `.new` is reserved for the no-arg / pre-allocated constructor |
| Other ambiguous constructors | follow §3.11 — `.new` / `.from` / `.from_iter` / `.with_capacity` |

### Tutorial-only language

| Old phrasing | New phrasing |
|---|---|
| `&self` / `&mut self` | "reading method" / "writing method" |
| "mutates the receiver" | "writes the receiver" |
| "mutating method" | "writing method" |
| "borrow mutably" | "borrow writably" |
| "borrow immutably" | "borrow read-only" |
| "trait" (as a Ruxen concept) | "mixin" |
| "impl block" | "extension block" or "method inside the type body" |
| "trait object" | "any-mixin existential" or "`any Mixin`" |

---

## Behavior changes (need judgment, not just s///)

These describe rule changes. If an existing doc or test asserts the
old behavior, fix the assertion to match the new rule. If you find
code that depends on the old behavior, leave a `TODO(migration):`
comment naming the affected behavior and continue — do not silently
"fix" semantics.

1. **Ambiguous mixin defaults are an error** (§3.4). Old docs may
   describe "later include wins"; the new rule emits
   `E-MIX-AMBIGUOUS-DEFAULT` and requires the class to define its
   own implementation.
2. **`nil` priority rule** (§3.10). Old "context infers" phrasing
   becomes a four-step priority. The diagnostic `E-NIL-AMBIGUOUS`
   fires when two candidate types are simultaneously well-typed.
   `E-NIL-RAW-OUTSIDE-UNSAFE` fires when a pointer-typed `nil`
   reaches outside an `unsafe` block.
3. **`Send` / `Sync` are auto-mixins**, not user-include forms
   (§3.6). Replace any user-written `include Send` with the auto
   rule; explicit forms are only `exclude Send` (opt out) or
   `unsafe include Send` (opt-in override).
4. **No `&var some/any Mixin`** (§3.4). To mutate through an
   existential, the receiver must be owning (`Box`, `Shared`,
   `SharedSync`); read-only borrows of existentials don't allow
   writing methods. If old code took `&mut dyn T`, rewrite to
   `Box[any T]` (or the right ownership form) or to a generic
   parameter `[T: Mixin]` with `&var T`.
5. **`extension C[T] where T: B`** is the form for conditional
   methods on a generic type (§3.4a). Per-method `where` clauses on
   individual `def`s are not a Ruxen form — re-group into an
   extension block if you see them.
6. **Variadic FFI is supported in v1** (§3.7). Old `out of scope
   (v2)` notes for `def f(fmt: *UInt8, ...)` should be deleted.
7. **`const` is only a generic-parameter prefix** (§3.14). Old
   `const NAME = ...` binding forms become module-level
   `let NAME = ...` — the compiler validates const-evaluability at
   the use site, not at the declaration.

---

## Where to sweep

Priority order — work top to bottom. Do not skip ahead.

### 1. Tutorials — `docs/tutorial/*.md` (25 files)

Highest user-facing surface. Apply every mechanical rule. Apply
behavior changes where the prose currently asserts the old rule.
Verify that every code block parses against the new syntax.

### 2. Stdlib specs — `docs/specs/stdlib/*.spec.md` (20 files)

The B-rows describe contracts. Apply the mixin/type renames; the
B-rows that mention `HashMap`, `HashSet`, `Hash` (mixin), should
move to `Map`, `Set`, `Hashable`. Check the Status header — if a
spec is "shipped Phase X" but its example code uses retired
syntax, fix the example.

### 3. System specs — `docs/specs/{ownership,mixins,codegen,system,types}/*.spec.md`

The file `docs/specs/mixins/system.spec.md` is written end-to-end
in `trait` / `impl T for U` / `dyn T` / `::` vocabulary. Rewrite
top-to-bottom. The file `docs/specs/mixins/implicit_includes.spec.md`
is titled "Spec — `derive <Trait>`" but should describe
implicit-includes; rename the title and rewrite. The file
`docs/specs/codegen/ffi.spec.md` uses `null` and `extern`; fix.

### 4. Requirements docs — `docs/requirements/*.md`

Many of these were written before the rename. Fix references in
prose and code blocks. Do not invent new requirements — only bring
existing text into line.

### 5. Fixture programs — `tests/release-e2e/cases/*.rx` + `expected/*.out`

Any fixture using retired syntax must compile under the new rules.
If a fixture exists to test a retired construct, decide whether to
delete it or rewrite it to test the equivalent new construct.

File **names** also need renaming: e.g. an old `06_let_mut.rx`
becomes `06_var.rx` (the git status at handoff shows this rename
is partial). Search for fixtures whose names contain `mut`,
`hash`, `trait`, `impl`, `dyn`, `comparable`, `displayable`,
`iterable`, `vec_`, `none`, and rename to the new vocabulary.

### 6. Rust source — `crates/ruxen-*/src/**/*.rs`

Lower priority but in scope for:
- **Error-message strings**: every user-visible diagnostic must use
  the new vocabulary. Search for `"trait"`, `"impl"`, `"dyn"`,
  `"HashMap"`, `"HashSet"`, `"Vec"`, `"&mut"`, `"`'a`"`, `"Hash"`
  (when referring to the mixin), `"Displayable"`, `"Comparable"`,
  `"Iterable"`, etc., in `format!` / `write!` / `Diagnostic::`
  construction sites.
- **Public identifiers** in `ruxen-core` that match retired surface
  vocabulary: e.g. `HirSelfMode::Mutating` should be renamed
  `Writing` if any error-message text uses the variant name. (If
  the variant is purely internal and never appears in user-facing
  text, leave it; the spec says internal naming is incidental.)
- **Doc comments** (`///` or `//!`) that describe surface syntax.

### 7. Top-level files

- `README.md` — examples and language tour
- `CHANGELOG.md` — any line referring to current syntax
- `CLAUDE.md` — if it references old forms
- `Cargo.toml` workspace metadata if it has tagline text
- `examples/**/*.rx` and `examples/**/README.md`

---

## What NOT to touch

- **`docs/specs/syntax/ruby-naming.spec.md`** itself. It is the
  destination. Leave it alone. The only exception is fixing typos
  you encounter while reading — flag any wording change in your
  output report.
- **The OLD column of the §10a migration table.** Those entries
  are *intentionally* the retired forms — they document what to
  migrate from. Do not "fix" them.
- **Archived migration plans** if any exist under
  `docs/specs/syntax/_migration-plan.md`. They reference both
  forms during transition; leave until the spec authors retire them.
- **Test fixtures whose stated purpose is to assert retired syntax
  is rejected.** A test named like `derive_keyword_is_rejected.rs`
  must keep using the literal token `derive` in its input source.
  Use judgment: is the file *exercising* old syntax (keep) or
  *demonstrating* old syntax (rewrite)?
- **Compiler-internal identifiers** that never appear in
  user-facing output. The spec §8 says internal naming is
  incidental. If renaming `HirSelfMode::Mutating` to
  `HirSelfMode::Writing` would touch 200 sites and zero of them
  surface to users, leave it and add a comment.
- **Third-party dependencies and vendored code.** Only Ruxen's own
  source is in scope.

---

## Verification protocol

After each priority area completes, run these checks. Do not move
to the next area if a check fails.

### After tutorials

```bash
# every retired token should be absent from docs/tutorial/
grep -rE '\bmut\b|\btrait\b|\bimpl\b|\bdyn\b|\bHashMap\b|\bHashSet\b|\b(Comparable|Displayable|Iterable)\b|\bNone\b|\bVec\[|\bRc\[|\bArc\[|::|@\[|let mut|&mut|\*mut' docs/tutorial/

# expected output: empty, or only lines that are (a) English
# prose ("immutable"/"mutable"/"mutation"), or (b) explicit
# "retired form" callouts. Inspect each remaining hit.
```

### After stdlib + system + codegen specs

```bash
grep -rE '\bmut\b|\btrait\b|\bimpl\b|\bdyn\b|\bHashMap\b|\bHashSet\b|\b(Comparable|Displayable|Iterable)\b|::' docs/specs/ \
  | grep -v 'ruby-naming.spec.md'

# Same expectation. The canonical spec is excluded from the grep.
```

### After source rewrites

```bash
cargo build --workspace
cargo test --workspace
# E2E fixtures (slow):
cargo test --release -p ruxen-core --test release_e2e_smoke -- --ignored
```

Every failing test points at a place where the spec and the
implementation disagree. Reconcile by following the spec; the
implementation is the side that gives.

### After fixture renames

```bash
# No fixture should be named with a retired token
find tests/release-e2e/cases -name '*.rx' \
  | grep -E '(_mut|_hash|_trait|_impl|_dyn|_vec|_comparable|_displayable|_iterable|_none)'

# expected output: empty
```

### Final cross-check

Re-grep the entire repo (excluding the canonical spec and your
own report). The only acceptable matches for retired tokens are:

1. English prose using "mutable/immutable/mutation" as adjectives
2. Test names like `xxx_rejects_old_keyword` whose body asserts
   rejection of the retired token literal
3. Comments explicitly marked `// migration: retained for...`

Everything else is drift you missed.

---

## Output

Produce a file `MIGRATION_REPORT.md` at the repo root with the
following sections:

1. **Summary** — count of files changed, by directory.
2. **Mechanical replacements** — one line per replacement family
   you applied; include count.
3. **Behavior-rule rewrites** — list every file where you rewrote
   prose/code to match a new rule (§3.4 / §3.10 / etc.), with a
   one-line diff summary.
4. **Renames** — file paths whose name changed.
5. **`TODO(migration)` comments left behind** — for cases you
   couldn't resolve automatically. Each entry: file:line + reason.
6. **Tests touched** — list of test files modified. For each,
   note whether the change was syntactic (s///) or semantic
   (asserts a different rule).
7. **Open questions** — anything where the canonical spec was
   silent and you had to make a call. Include your choice and
   one-line rationale.
8. **Verification log** — the grep / cargo / test output that
   proves the repo is clean.

Do not commit changes. Leave the work uncommitted so a human can
review the report and the diff before merging.

---

## Constraints

- **Do not invent syntax.** If a doc references a form not in the
  canonical spec, flag it under Open Questions; do not guess.
- **Do not modify the canonical spec** (`ruby-naming.spec.md`) or
  this migration prompt.
- **Do not skip the verification protocol.** Each priority area's
  grep / build / test step is mandatory.
- **Be conservative on Rust source.** When in doubt about whether
  an internal identifier is user-visible, leave it and note it.
- **Be aggressive on user-facing surfaces** (tutorials, specs,
  examples). These are the contract.

Begin with priority 1 (tutorials). Read the canonical spec first.
