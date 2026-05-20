# Spec — Compiler consolidation: single entry per concept

**Source docs:**
[docs/specs/system/zero_rust_stdlib_classes.spec.md](zero_rust_stdlib_classes.spec.md)
(task #14 closed the trio leak; this task removes the underlying
duplication that caused it).

**Status:** new — methodical refactor pass. Every "two paths for one
concept" pair surfaced during multithreading + async work gets
collapsed into one entry point with a canonical comment marking it
as the only place that handles X.

**Principle:** when the same conceptual operation has multiple code
paths, each new feature has to touch (and risk breaking) all of
them. Consolidating them locally is more important than abstract
"cleanup" — the diagnosis already exists from this session.

---

## B1 — Method dispatch in `mir/lower/expr/method_call.rs`: one path

Today three branches handle `expr.method(args)` lowering:

1. `is_collection_ctor` fast path (for `.new`, `.with_capacity`, etc.) — keyed on a hardcoded class-name allowlist.
2. `is_user_static_method` branch (for `def self.X` lib decls).
3. Instance-method branch (for `def X(self, ...)` lib decls).

Commit `8d5f10a` had to patch TWO of them to consult the FFI alias map; future contributors will face the same fan-out.

**Fix:** collapse to ONE function `lower_method_call_via_ffi_alias` that:
1. Composes the alias-map key from `<receiver_type>_<method_name>` (with generic-strip fallback).
2. Looks up the FFI alias map.
3. If found: emits the call with the user's explicit args; prepends `self` iff the registered signature includes a receiver type.
4. If not found: falls back to the existing "no FFI alias → builtin handler / Class_init synthesis" paths.

The three pre-existing branches become callers of this function, OR (preferred) the function replaces them where the semantics are identical.

**Acceptance:** the file has ONE entry point that consults the FFI alias map. A canonical comment marks it. Existing regression tests (the trio-leak pin, 700–724 e2e, multithreading construction-site checks) pass unchanged.

## B2 — `hir/types.rs`: collapse `is_send_with` and `is_send_strict_with`

Task #14 commit `793751c` rewrote `is_send_strict_with` as a generic walker. But `is_send_with` (the looser variant for user-class auto-derive) still exists alongside it.

**Fix:** one entry point `is_send(ty, symbols) -> bool` with the rules driven by mixin-membership data in the resolver:
- `include !Send` → false.
- `include unsafe Send` → true (escape hatch).
- `include Send` → recursive walker (generic args + fields, with `subst_type_params` for substituting at call sites).
- Otherwise → false.

Same shape for `is_sync`. The two-function legacy gets one canonical entry; the "strict" variant becomes the only behaviour. (User classes WITHOUT `include Send` are not Send, which is the spec'd rule from `send_sync_enforcement.spec.md` §B10 — there was never supposed to be a "looser" variant.)

**Acceptance:** one function. One comment. All Send/Sync sites in the codebase route through it. The four `auto_derive_send_sync.rs` pin tests still pass.

## B3 — Bootstrap vs user-program type registration: shared entry

Task #14 commit `b74546d` ran `resolve_item` over bootstrap programs in pass 2. But the type-registration entry point (`register_top_level_type_with_ffi_in`) is still called from two contexts (bootstrap merge + user program) with slightly different surrounding state.

**Fix:** make registration genuinely path-agnostic. The function should take the symbol-table state as input, not assume "user-program mode" vs "bootstrap mode". Eliminate any branches inside that check which caller invoked it. The work in `bootstrap_merge.rs` should be reduced to "set up the cumulative symbol table, then route through the same registration path the user program uses."

**Acceptance:** `register_top_level_type_with_ffi_in` has no conditional logic on caller identity. The full type-registration flow can be exercised by a fresh `.rvn` package without any bootstrap-specific glue. The bootstrap_class_bodies pin test (added in #14) still passes.

## B4 — Drop registration: `def drop` and `include Drop` agree

`collect_user_drop_classes` in `mir/lower.rs` matches the literal method name `drop`. There's also a `Drop` mixin that user code can `include`. These two mechanisms aren't unified — adding `include Drop` to a class doesn't automatically register a Drop hook; the user must ALSO define `def drop`.

**Fix:** one source of truth. Either:
- (a) `include Drop` is the only registration trigger; the `def drop` body is the implementation; `collect_user_drop_classes` walks classes that include Drop AND have a `def drop` method.
- (b) `def drop` is the only registration trigger; `include Drop` becomes a no-op marker.

Pick (a) — it matches the existing `include Display` / `include Clone` pattern where the mixin is the contract and `def method` is the implementation. Code without `include Drop` but with `def drop` should emit a warning (E07XX) suggesting the include.

**Acceptance:** ONE registration mechanism. Existing `drop_fixtures.rs` tests still pass (or the pre-existing failures are fixed as part of this work — confirm via baseline diff). A new pin test asserts `include Drop` without `def drop` is rejected, and `def drop` without `include Drop` warns.

## B5 — Acceptance criteria (per consolidation)

For each B-row, the commit must:
1. Have ONE entry point handling the concept (function-scope or module-scope, whichever is natural).
2. Carry a canonical comment at the entry point: `// SINGLE ENTRY POINT for <concept>. Adding new <feature kind> means changing only this function.`
3. All existing tests in the affected area pass.
4. A new pin test (or extension of existing) asserts the entry point is the only place — e.g. `grep -rn "<old_function_name>" compiler/riven_core/src/` returns only the new entry's caller.

## Out of scope

- **Codegen Cranelift vs LLVM dispatch tables.** The two backends have separate dispatch tables for runtime symbols. Worth examining as a separate task (`#15-codegen`); the current task focuses on resolver / typeck / MIR consolidation where the diagnosis is concrete.
- **`is_collection_ctor` removal entirely.** B1 reduces it to a wrapper / removes it where the FFI-alias path subsumes it. Full removal may require removing some legacy stdlib classes from its allowlist — defer if it cascades.
- **Async-lowering passes (`async_lowering/mod.rs`).** Legitimately a single dedicated lowering pass; not duplicated. No consolidation needed there.

## Suggested sequence

1. **B2 first** (Send/Sync collapse). Smallest delta, builds on commit `793751c`. ~10 min.
2. **B4** (Drop registration). Medium effort but isolated. Surfaces and ideally resolves the pre-existing `drop_fixtures.rs` failures.
3. **B3** (bootstrap vs user-program). Medium. Builds on `b74546d`. Risk: the cumulative symbol table state may have caller-specific gotchas.
4. **B1 last** (method dispatch). Highest impact, highest risk. The dispatch path is load-bearing; mistakes break every method call. Land after B2/B3/B4 so the regression surface is established.

Each is its own commit. Per rule 42, narrow tests only — the 4 new test suites added in task #14, the multithreading regression e2e (700/703/710/711/712), and the async pin suite. The full workspace pass is end-of-phase.

---

## Pin tests

The acceptance criterion already specifies per-B-row pin tests. Concretely:
- `mir_method_call_single_entry_point` — assert there's one entry consulting FFI alias map.
- `send_sync_single_function` — `grep` count for is_send_with returns 1 (the new entry).
- `bootstrap_user_type_registration_path_agnostic` — exercise the same registration shape from both contexts and assert equivalent state.
- `drop_registration_single_mechanism` — `include Drop` + `def drop` both required; warn if mismatched.

---

## Out of scope (follow-up tasks)

- Cranelift vs LLVM dispatch consolidation (#15-codegen).
- `register_builtins` removal — moving every hardcoded built-in to `.rvn` (paired with the BOOTSTRAP_FILES load-check rule from memory note `project_riven_bootstrap_files_load_check.md`).
- Auto-derive for other built-in traits (Clone, Display, Hash) — task #14 only did Send/Sync.
