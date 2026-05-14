# 01 — Phase 1 remainder

Closes the four leftover items from Phase 0/1: P0.7 string/array drops,
T1.05 builtin derive generators, T5.04 phase 3 long-form explanations,
and P0.12 un-reservation of actor tokens.

**Depends on:** Phase 0 done (squashed into commits prior to `master@{HEAD}`; verified 2026-05-05 against substantive evidence — LICENSE files, MSRV pin, attr untangle fixtures, doc-comment capture, Drop infra, variance fixtures all present).
**Reads:** `docs/requirements/tier1_04_drop_copy_clone.md`,
`docs/requirements/tier1_05_implicit_includes.md`,
`docs/requirements/tier5_04_error_code_registry.md`.

---

## A. P0.7 — built-in drops for `String` / `Array` / `Map`

### Problem
String literals lower to `riven_string_from(...)` (heap copy) — owned.
But `mir/lower.rs:6082` skips drop for non-Class/Struct/Enum types,
so heap-owned `String`/`Array`/`Map` locals leak at scope exit.

### TDD
1. Extend `tests/drop_fixtures.rs` with three failing tests:
   - `string_local_freed_at_scope_exit` — `let s = String.from("x"); ...`
     asserts allocs == frees.
   - `array_local_freed_at_scope_exit` — `var v = Array.new` + push.
   - `map_local_freed_at_scope_exit` — `var m = Map.new`.
2. Run; confirm they fail with `outstanding > 0`.

### Implementation
- Widen the heap-owned predicate in `insert_drops`
  (`mir/lower.rs:6058-6088`) to include the internal type-tags for
  `String`, `Array`, and `Map`.
- Add type-specific drop callees so the runtime walks owned
  pointers. Wire `String_drop`, `Array_drop`, `Map_drop` runtime
  functions in `crates/riven-core/runtime/runtime.c` (free element
  storage + outer struct).
- Update `compute_dealloc_safe_locals` to permit these types.
- Confirm `riven_string_from` is the canonical heap-owning
  constructor and that string-literal codegen at
  `mir/lower.rs:4691` (`emit_owned_string_literal`) routes through
  it. No raw `.rodata` pointer must reach `riven_dealloc`.

### Definition of done
- [x] Three new drop fixtures green.
- [x] No `runtime_no_leak` regression.
- [x] `cargo test --workspace` green.
- [x] CHANGELOG bullet.

---

## B. T1.05 — built-in derive generators

### Problem
`derive Debug`, `derive Clone`, `derive Copy`, `derive PartialEq`,
`derive Eq`, `derive Hash`, `derive Default`, `derive Ord`,
`derive PartialOrd` are validated (`implicit_includes/mod.rs`) but no
include-blocks are synthesized.

### TDD
For each derive mixin, add a fixture pair:

```
tests/release-e2e/cases/2NN_derive_debug_struct.rvn
tests/release-e2e/cases/2NN_derive_clone_struct.rvn
tests/release-e2e/cases/2NN_derive_copy_struct.rvn
... etc
```

Each fixture exercises the derived behavior on one struct, one enum,
one nested enum-with-payload. Expected output asserts:

- `Debug` → exact `"Foo { x: 1, y: 2 }"` shape.
- `Clone` → independent copy (mutate one, read other unchanged).
- `Copy` → bit-copy semantics, source still usable.
- `PartialEq` / `Eq` → `==` comparisons.
- `Hash` → equal values produce equal hashes (use existing
  Map as proxy).
- `Default` → field-wise zero/`Default.default()`.
- `Ord` / `PartialOrd` → field-order tuple semantics.

Add unit tests in `crates/riven-core/src/implicit_includes/` covering the HIR
synthesis (assert that `derive Debug` on `Point { x: Int, y: Int }`
produces a `Point_fmt_debug` HIR method with the right shape).

### Implementation
- Generate HIR include-items in a new pass `derive::synthesize` that
  runs after `validate_program` and before `lower`.
- Each derived mixin emits one synthesized include for `<Mixin>` on
  `<Type>` with method bodies built from field iteration.
- Reuse runtime helpers where available; for `Debug` use new
  `riven_string_concat` runtime fn (already in `runtime.c`).
- Reject derive on types whose fields don't themselves satisfy the
  mixin — emit `E0610-E0618` per the namespace.

### Definition of done
- [x] All 9 builtin derives generate working includes.
- [x] Per-derive fixture pair under `tests/release-e2e/cases/`.
- [x] Negative tests for "field doesn't satisfy bound" cases.
- [x] `cargo test --workspace` green.
- [x] All new error codes in `diagnostics/codes.rs::REGISTRY`.
- [x] CHANGELOG bullet.

---

## C. T5.04 phase 3 — long-form explanations

### Problem
`riven explain ECODE` only prints the title. Rust's `--explain`
prints multi-paragraph explanation + example.

### TDD
1. Add `tests/explain_long_form.rs` integration test that runs
   `riven explain E0001` and asserts stdout contains both the title
   AND a "## Example" section.
2. Add unit test in `riven-cli/src/explain.rs` for
   `load_explanation(code)` returning `Some(content)` when a markdown
   file exists at `docs/errors/<code>.md`.

### Implementation
- Create `docs/errors/E0001.md` ... `docs/errors/E1014.md` (one per
  registered code). Each file:

  ```markdown
  # E0001: unterminated block comment

  ## Why
  <one paragraph>

  ## Example
  ```rvn
  <triggering snippet>
  ```

  ## Fix
  <how to resolve>
  ```

- Embed all `docs/errors/*.md` into the `riven` binary via
  `include_str!` in `riven-cli/src/explain.rs`.
- `explain()` falls back to title-only when no markdown found.

### Definition of done
- [x] `docs/errors/<code>.md` for every entry in REGISTRY.
- [x] `riven explain E0001` prints title + Why + Example + Fix.
- [x] Test ensuring every REGISTRY entry has a matching markdown file.
- [x] CHANGELOG bullet.

---

## D. P0.12 — un-reserve actor tokens (commit to async-only path)

### Problem
`actor`, `spawn`, `send`, `receive` are reserved in lexer/token.rs
but parser never consumes them. Decision (per session 2026-04-29):
ship async, defer actors to v2 as library, then language v2.0.
Un-reserve the four tokens now to free user-level identifier names.

Keep `async`/`await` reserved — those land in Phase 4 prompt 15.

### TDD
1. Add `tests/release-e2e/cases/2NN_unreserved_actor_idents.rvn`:

   ```riven
   def main
     let actor = 1
     let spawn = 2
     let send = 3
     let receive = 4
     puts "#{actor + spawn + send + receive}"
   end
   ```

   Expected output: `10`.

2. Confirm test fails today (parser rejects `actor` as keyword).

### Implementation
- Remove `Actor`, `Spawn`, `Send`, `Receive` from `TokenKind` in
  `lexer/token.rs`.
- Remove their `lookup_keyword` entries.
- Search for any parser branch that referenced these tokens and
  delete (none should exist if audit was correct).
- Update `135_unreserved_idents.rvn` to also cover the four.

### Definition of done
- [x] All four tokens un-reserved.
- [x] No parser regressions.
- [x] New e2e fixture green.
- [x] CHANGELOG bullet noting deferred actor decision.

---

## Universal DoD (also)

- [x] All four sub-items merged in dependency order: A → B → C → D.
- [x] `cargo test --workspace` green (CI gate stand-in; PR review deferred per user rule).
