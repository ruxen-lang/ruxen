# Phase 6 — Cleanup Sweep (derive / MethodCall / remaining walkers / cheap wins) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`)
> syntax for tracking. This is Phase 6 of `2026-06-04-thermonuke-master.md` and runs AFTER Phase 1 —
> it consumes Phase 1's already-merged `parser::visit::{Visit, VisitMut, walk_expr, walk_block,
> walk_stmt}` and `hir::types::Ty::map_inner` as existing APIs.

**Goal:** A sweep of four independent cleanup buckets, each its own task group: (a) collapse the
duplicated per-primitive debug-format ladder in `derive.rs` onto the existing-but-bypassed
`format_field_for_debug`, then DRY the eq/hash/cmp/clone/default field-walks behind one
`fold_struct_fields` driver; (b) split the ~300-line `MethodCall` god-arm in `typeck/infer/expr.rs`
into three focused helpers and delete the harvest block that duplicates `collect.rs`; (c) migrate the
remaining hand-rolled walkers (repl `eval.rs` ×8, `formatter/comments.rs` span walk, `monomorphize.rs`
`walk_tys_in_*`) onto Phase 1's primitives; (d) cheap wins — one shared `COLLECTION_BUILTINS` const,
rename the `library/std/foobar` pin fixture to a self-describing name, and record the
`ruxen_noop_passthrough` decision. Every migration is behaviour-preserving and guarded by a
characterization test.

**Architecture:** No new abstraction is invented here beyond `fold_struct_fields` (bucket a) —
everything else either *reuses* a Phase 1 primitive or *deletes* a duplicate. `fold_struct_fields` is
introduced only because there are FIVE existing callers of the identical per-field GetField+combine
skeleton (eq/hash/cmp/clone/default), so it clears YAGNI's "third caller" bar by a wide margin. The
`MethodCall` split is pure extraction (no behaviour change): three private `infer_*` methods carved
out of one arm, sharing the existing `select_class_method`/`resolve_method_call`/`builtin_method_type`
seams. Bucket (c) is migration onto Phase 1; bucket (d) is text-level dedup + a rename + a documented
decision.

**Tech Stack:** Rust 1.91, `cargo test -p ruxen_core` and `cargo test -p ruxen_repl`. No new
dependencies.

> **Per-task direction-check (maintainer-mandated):** after the commit step of EVERY task below, run
> the `thermonuke` skill scoped to that task's diff: invoke it with arg `git diff HEAD~1..HEAD` and
> confirm (a) lines moved in the intended direction (net reduction), (b) no new `_ =>` catch-all in any
> traversal/table, (c) no new god-function/special-case in a shared path, (d) the task's structural
> goal was met (a hand-rolled walker / a duplicate was *deleted*, not added). If it flags drift, STOP
> and surface it. The full multi-agent sweep runs in the final-integration task. Each task's checkbox
> list ends with a `- [ ] Direction-check (/thermonuke on this task's diff)` step.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `compiler/ruxen_core/src/mir/lower/derive.rs` | struct `_to_debug`/eq/hash/cmp/clone/default synth; `format_field_for_debug` helper | Modify (route debug→helper; add `fold_struct_fields`) |
| `compiler/ruxen_core/src/typeck/infer/expr.rs` | `MethodCall` inference arm | Modify (split into 3 helpers, delete dup harvest) |
| `src/ruxen_repl/src/eval.rs` | 8 hand-rolled AST walkers | Modify (migrate to `parser::visit`) |
| `compiler/ruxen_core/src/formatter/comments.rs` | `visit_expr` span-collection AST walk | Modify (migrate to `parser::visit::Visit`) |
| `compiler/ruxen_core/src/mir/lower/monomorphize.rs` | `walk_tys_in_*` HIR `Ty`-collector | Modify (shared HIR-Ty visitor; remove `_ =>` arms) |
| `compiler/ruxen_core/src/resolve/ffi_registration.rs` | anchor-only-builtin membership test | Modify (use `COLLECTION_BUILTINS`) |
| `compiler/ruxen_core/src/resolve/types.rs` | `resolve_type_expr` collection arms + (new) `COLLECTION_BUILTINS` const | Modify (define const, share with ffi) |
| `compiler/ruxen_core/src/resolve/bootstrap.rs` | `BOOTSTRAP_FILES` list | Modify (rename foobar path entry) |
| `compiler/ruxen_core/src/resolve/stdlib_embedded.rs` | embedded-stdlib include table | Modify (rename foobar `include_str!` entry) |
| `library/std/foobar/` → `library/std/_pin_zero_rust_stdlib/` | trio-leak pin fixture package | **Rename** (`git mv` + Ruxen.toml name) |
| `compiler/ruxen_core/tests/trio_leak_pin.rs` | B5 trio-leak pin assertions | Modify (update package name / namespace / substring) |
| `compiler/ruxen_core/src/codegen/lang_intrinsics.rs` (read-only) | `ruxen_noop_passthrough` intrinsic alias | Document decision (no code change expected) |

**Module placement rationale:** `fold_struct_fields` lives next to its five callers in `derive.rs`.
The three `infer_*` helpers stay private methods on the inference engine in `expr.rs` (they only carve
existing logic). `COLLECTION_BUILTINS` lives in `resolve/types.rs` (the type-resolver owns the
canonical collection vocabulary) and is `pub(crate)` so `ffi_registration.rs` imports it.

> **Phase-1 dependency note (load-bearing):** Buckets (b)/(c) reference `parser::visit::{Visit,
> VisitMut, walk_expr, walk_block, walk_stmt}` and `Ty::map_inner` as already-merged. If Phase 1 is not
> yet on the branch, STOP — Phase 6 cannot start. Bucket (a) and bucket (d) are independent of Phase 1
> and may proceed regardless.

---

## Bucket (a) — `derive.rs`: route struct-debug through the bypassed helper, then DRY the field-walks

### Task 1: Make `synthesize_struct_to_debug` call `format_field_for_debug`

**Files:**
- Modify: `compiler/ruxen_core/src/mir/lower/derive.rs` — the per-field branch inside
  `synthesize_struct_to_debug` (lines ~200-251) and its caller body (~160-260).
- Test: `compiler/ruxen_core/tests/implicit_debug_formats_struct.rs` is the existing pin; add a focused
  characterization assertion there if one isn't already present.

**Why:** `synthesize_struct_to_debug` (derive.rs:160-251) hand-inlines the EXACT per-primitive ladder
(`Char`→`ruxen_char_to_string`, integer→`ruxen_int_to_string`, float→`ruxen_float_to_string`,
`Bool`→`ruxen_bool_to_string`, `String|Str`→identity, nested-derive-Debug struct→`{name}_to_debug`,
else→`"<...>"`) that `format_field_for_debug` (derive.rs:1102-1174) ALREADY implements — and the
helper is strictly *more* complete (it also handles `enum_with_derive_debug`, which the inline copy at
line 234 omits). The inline copy is dead-by-duplication.

- [ ] **Step 1: Write/confirm the characterization test**

In `compiler/ruxen_core/tests/implicit_debug_formats_struct.rs`, ensure there is a test that compiles
and runs a struct with at least one of each: an integer field, a `String` field, a `Bool` field, and a
field whose type is another `derive Debug` struct, then asserts the rendered `_to_debug` output is the
expected `Name { f1: <v1>, f2: <v2>, ... }` string. If the file already has `derive_debug_struct_*`
coverage for these, add ONE new case that also includes a field of a nested `enum`-with-`derive Debug`
type — because the inline ladder at derive.rs:234 currently renders such a field as `<...>` while the
helper renders it via `{enum}_to_debug`. This case pins the BEHAVIOUR IMPROVEMENT the migration causes
(the only intentional behaviour change in this task) and must be asserted to the helper's output.

- [ ] **Step 2: Run to verify the nested-enum case fails (and the rest pass)**

Run: `cargo test -p ruxen_core --test implicit_debug_formats_struct 2>&1 | tee tmp/test-cache/phase6-task1-red.log`
Expected: the existing primitive cases PASS (pinning current good behaviour); the new nested-enum-field
case FAILS — current inline ladder emits `<...>` for it. (If the harness can't yet construct a nested
enum field, fall back to asserting current behaviour for all cases and treat the task as pure
behaviour-preserving dedup — note that choice in the commit message.)

- [ ] **Step 3: Replace the inline ladder with a call to the helper**

In `synthesize_struct_to_debug` (derive.rs), delete the `let field_str = if field.ty == Ty::Char {
... } else if ... else { "<...>" }` block (lines ~200-251) and replace it with:

```rust
            let field_str = self.format_field_for_debug(&mut mir_fn, entry, field_local, &field.ty);
```

Keep everything else (the `"{name}: "` label emission, the `", "` separator, the leading/trailing
brace emission, the `acc`/`string_concat` chaining) byte-for-byte. The helper takes
`(&mut MirFunction, BlockId, LocalId, &Ty)` and returns the `LocalId` of the formatted-field string —
the same value the inline block produced into `field_str`.

> Implementer: confirm the helper's borrow shape composes — `format_field_for_debug` takes
> `&mut MirFunction` while the surrounding loop also pushes into `mir_fn.blocks[entry]`. The helper
> already does its own `mir_fn.blocks[block]` pushes, so pass `entry` as `block`. There is no aliasing
> issue (sequential `&mut` borrows, not simultaneous).

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p ruxen_core --test implicit_debug_formats_struct 2>&1 | tee tmp/test-cache/phase6-task1-green.log`
Expected: PASS including the nested-enum case (now rendered via `{enum}_to_debug`).
Then the enum-side pins (the helper is shared with enum debug paths):
`cargo test -p ruxen_core --test implicit_debug_formats_enum --test enum_codegen_debug --test enum_mir_debug 2>&1 | tee tmp/test-cache/phase6-task1-enum.log`
Expected: PASS (no regression — these never used the deleted inline copy).

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/mir/lower/derive.rs compiler/ruxen_core/tests/implicit_debug_formats_struct.rs
git commit -m "refactor(derive): struct debug uses format_field_for_debug helper

synthesize_struct_to_debug hand-inlined the per-primitive format ladder
that format_field_for_debug already implements (more completely — the
inline copy rendered nested derive-Debug enums as <...> instead of
calling {enum}_to_debug). Route the struct path through the helper,
deleting the duplicate ladder.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

### Task 2: Introduce `fold_struct_fields` and share it across eq/hash/cmp/clone/default

**Files:**
- Modify: `compiler/ruxen_core/src/mir/lower/derive.rs` — `synthesize_struct_eq` (283-357),
  `synthesize_struct_hash_code` (359-~430), and the `cmp`/`clone`/`default` synth fns (find each by
  `fn synthesize_struct_{cmp,clone,default}` / `synthesize_default_value` neighbours).
- Test: `compiler/ruxen_core/tests/implicit_partial_eq_returns_correct.rs` +
  `implicit_debug_formats_struct.rs` (clone/default coverage). Add focused cases as needed.

**Why:** All five fns share the identical skeleton: iterate `s.fields.iter().enumerate()`, emit a
`GetField { dest, base: self_local, field_index: idx }` (and for eq/cmp a second on `other_local`),
compute a per-field value via a type-dispatched callback, and fold the per-field results into an
accumulator. Only (1) the per-field callback and (2) the fold operator differ. This is a textbook
single-driver-with-a-closure case — five callers, far past the YAGNI threshold.

- [ ] **Step 1: Write the characterization tests FIRST**

These pin the CURRENT output of all five derived methods before extraction. In
`implicit_partial_eq_returns_correct.rs` (eq) and `implicit_debug_formats_struct.rs` (or a sibling that
exercises clone/default), ensure there is at least one struct that:
- has two integer fields and a `String` field (exercises the primitive + string-eq/string-hash arms),
- derives `PartialEq`, `Hashable`, `Comparable`, `Clone`, and `Default`,
and asserts the runtime behaviour of `==`, `hash_code`, `<`/`cmp`, `clone`, and `Default.new` (or the
project's default-construction surface). If clone/default lack an existing integration harness, add a
minimal one mirroring the eq test's compile-and-run helper.

- [ ] **Step 2: Run to verify the characterization tests pass (baseline GREEN)**

Run: `cargo test -p ruxen_core --test implicit_partial_eq_returns_correct --test implicit_debug_formats_struct 2>&1 | tee tmp/test-cache/phase6-task2-baseline.log`
Expected: PASS. This is a REFACTOR task — the tests are green before AND after; they exist to catch a
behaviour change introduced by the extraction. (There is no red phase here; the red→green discipline
is satisfied by Task 1's nested-enum case and by buckets b/c. Per the master plan, behaviour-preserving
extractions are pinned by characterization tests, not bug-exposing ones.)

- [ ] **Step 3: Add the `fold_struct_fields` driver**

Add a private method to `derive.rs` near the other synth helpers. Signature (the implementer adapts the
exact closure shapes to what eq/cmp need — TWO field locals — vs hash/clone/default — ONE):

```rust
    /// Drive the shared per-field walk used by every derived struct method.
    /// For each field it emits the `GetField` load(s) and calls `per_field`
    /// with the field index, its type, and the loaded local(s); `per_field`
    /// returns the LocalId of this field's contribution. The contributions are
    /// folded left-to-right by `combine`, seeded with `init`. Eq/Cmp pass
    /// `other_local = Some(..)` to also load the rhs field; Hash/Clone/Default
    /// pass `None`.
    fn fold_struct_fields(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        s: &HirStructDef,
        self_local: LocalId,
        other_local: Option<LocalId>,
        init: LocalId,
        mut per_field: impl FnMut(&mut MirFunction, usize, &Ty, LocalId, Option<LocalId>) -> LocalId,
        mut combine: impl FnMut(&mut MirFunction, LocalId, LocalId) -> LocalId,
    ) -> LocalId {
        let mut acc = init;
        for (idx, field) in s.fields.iter().enumerate() {
            let lhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::GetField {
                dest: lhs, base: self_local, field_index: idx,
            });
            let rhs = other_local.map(|ol| {
                let r = mir_fn.new_temp(field.ty.clone());
                mir_fn.blocks[block].instructions.push(MirInst::GetField {
                    dest: r, base: ol, field_index: idx,
                });
                r
            });
            let contribution = per_field(mir_fn, idx, &field.ty, lhs, rhs);
            acc = combine(mir_fn, acc, contribution);
        }
        acc
    }
```

> Implementer obligations (explicit sub-steps, not placeholders):
> - **eq** (derive.rs:303-353): `per_field` = the `field_eq` `if struct_with_derive_trait("PartialEq")
>   { {name}_eq } else if String|Str { ruxen_string_eq } else { Compare Eq }` block; `combine` = the
>   `BinOp::And` step; `init` = the `Literal::Bool(true)` seed; `other_local = Some(other_local)`.
> - **hash** (derive.rs:377-~420): `per_field` = the `field_hash`
>   `if struct_with_derive_trait("Hashable"|"Hash") { {name}_hash_code } else if String|Str {
>   ruxen_string_hash } else { field_local }` block; `combine` = the existing FNV `BitXor`-then-`Mul`
>   pair (the implementer keeps both ops — `combine` may emit two instructions and return the final
>   `next`); `init` = the FNV offset basis `1469598103934665603`; `other_local = None`.
> - **cmp / clone / default**: read each fn's body and map its per-field block + accumulator op the same
>   way. `default` does not load `self` fields (it constructs) — if its skeleton diverges (it builds a
>   value rather than folding loaded fields), LEAVE IT as-is and do NOT force it through the driver.
>   Only migrate the fns whose skeleton genuinely matches (eq/hash/cmp/clone at minimum). YAGNI: do not
>   contort `fold_struct_fields` to swallow `default` if it doesn't fit — three-to-four real callers
>   already justify the driver. Record in the commit which fns were migrated.

- [ ] **Step 4: Rewrite each migrated synth fn to call `fold_struct_fields`**

Replace each fn's manual `for (idx, field)` loop with a `self.fold_struct_fields(...)` call whose two
closures carry the fn-specific `per_field`/`combine`. The fn's prologue (param setup, `init` temp,
terminator/return) stays. Net per fn: the ~30-50-line loop collapses to one call + two short closures.

- [ ] **Step 5: Run to verify green (output unchanged)**

Run: `cargo test -p ruxen_core --test implicit_partial_eq_returns_correct --test implicit_debug_formats_struct 2>&1 | tee tmp/test-cache/phase6-task2-green.log`
Expected: PASS (identical to baseline). Diff the two logs:
`diff <(grep -E "test result|FAILED|ok\b" tmp/test-cache/phase6-task2-baseline.log) <(grep -E "test result|FAILED|ok\b" tmp/test-cache/phase6-task2-green.log)`
Expected: no differences.

- [ ] **Step 6: Commit**

```bash
git add compiler/ruxen_core/src/mir/lower/derive.rs compiler/ruxen_core/tests/implicit_partial_eq_returns_correct.rs compiler/ruxen_core/tests/implicit_debug_formats_struct.rs
git commit -m "refactor(derive): fold_struct_fields driver shared by eq/hash/cmp/clone

Five derived struct methods hand-rolled the identical per-field
GetField+combine skeleton. Extract one fold_struct_fields driver
parameterised by a per-field callback and a combine op. Output is
byte-identical (characterization tests unchanged red→green).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

## Bucket (b) — `typeck/infer/expr.rs`: split the `MethodCall` god-arm, delete the dup harvest

### Task 3: Extract `infer_constructor_call` / `infer_selected_method` / `infer_combinator_block`; delete the harvest dup

**Files:**
- Modify: `compiler/ruxen_core/src/typeck/infer/expr.rs` — the `HirExprKind::MethodCall` arm
  (279-582). The constructor-call block is 476-502; the selected-method return block is 503-547; the
  duplicated generic-harvest is 526-545; the block-combinator unify is 562-579.
- Reuse (no change): `collect.rs:855` `bind_type_params_from_args` (the harvest the dup mirrors).
- Test: `compiler/ruxen_core/tests/regex_typeck.rs` is the existing pin; add focused inline/integration
  characterization cases for constructor inference and the `expect[T]` harvest.

**Why:** The arm is ~300 lines doing four separable jobs: (1) the `entry().or_insert()` chain special
case (297-381), (2) constructor-`new` generic inference (479-502), (3) selected-declared-method return
typing incl. a generic harvest (503-547), (4) block-combinator element-type unification (562-579).
Sub-block 526-545 RE-IMPLEMENTS the `{T → concrete}` harvest that `collect.rs`'s
`bind_type_params_from_args` (855+) already provides — it even builds the same `param_names` HashSet
and calls `Self::bind_type_params_from_args` then `Self::subst_ty`. The dup is the harvest *driver*
loop, not the recursion; collapse it to a single shared call.

- [ ] **Step 1: Write the characterization tests FIRST**

The arm is behaviour-dense; pin the four jobs before touching them. Add to `regex_typeck.rs` (or a new
`method_call_inference.rs` integration test mirroring its harness) cases asserting the INFERRED TYPE
of:
- `Pair.new(42, "hi")` ⇒ `Pair[Int, String]` (constructor generic inference, job 2),
- a declared `def expect[T](actual: T) -> Matcher[T]` called `expect(aString)` ⇒ `Matcher[String]`
  (the harvest, job 3 — this is exactly the case the dup's comment at 519-525 cites),
- `opt.map { |n| n * 2 }` on `Option[Int]` ⇒ `Option[Int]` (block-combinator unify, job 4),
- `m.entry(k).or_insert(v)` typechecks to `Unit` and rejects `x.or_insert(v)` with the
  "requires an immediate `.entry(K)` receiver" diagnostic (job 1, the special case — keep it intact).

- [ ] **Step 2: Run to verify baseline GREEN**

Run: `cargo test -p ruxen_core --test regex_typeck 2>&1 | tee tmp/test-cache/phase6-task3-baseline.log`
Expected: PASS. (Refactor task — green before and after. If a new case reveals a pre-existing bug,
STOP and surface it; do not fold a bug fix into this extraction.)

- [ ] **Step 3: Extract the three helpers**

Add three private methods on the inference engine (same `impl` block as the `MethodCall` arm lives in).
Signatures the implementer finalizes against the surrounding types (`&mut self`, the resolved
`derefed: &Ty`, `method_name: &str`, `args: &mut [HirExpr]`, `span: &Span`, and the
`selected_method: Option<DefId>` already computed at 449-456):

- `fn infer_constructor_call(&mut self, derefed: &Ty, method_name: &str, args: &[HirExpr], span: &Span)
  -> Ty` — move the `method_name == "new"` body (486-502): the `Ty::Class { generic_args.is_empty() }`
  branch that calls `infer_class_generics` or falls back to `resolve_method_call`.

- `fn infer_selected_method(&mut self, selected: DefId, derefed: &Ty, args: &[HirExpr], span: &Span)
  -> Ty` — move the selected-method return-typing body (503-547): fetch the signature, unify args,
  `wrap_async_return`, `substitute_generics_in_return`, THEN the harvest. **Replace** the inline
  harvest driver (526-545) with a single call into a small shared helper (see Step 4).

- `fn infer_combinator_block(&mut self, method_name: &str, block: &Option<Box<HirExpr>>, ret_ty: &Ty,
  span: &Span)` — move the `method_name == "map"` block-element unification (562-579). Returns nothing;
  it unifies in place.

Then the `MethodCall` arm body shrinks to: the entry-chain special case (unchanged), the
`infer_expr(object)` / arg / generic_args resolution, the `collect` early return, the closure-param
seeding, `selected_method` selection, and a `ret_ty` computed by:

```rust
                let ret_ty = if let Some(ret) = builtin_ret {
                    ret
                } else if method_name == "new" {
                    self.infer_constructor_call(&derefed, method_name, args, &expr.span)
                } else if let Some(selected) = selected_method {
                    self.infer_selected_method(selected, &derefed, args, &expr.span)
                } else {
                    let raw = self.resolve_method_call(&derefed, method_name, args, &expr.span);
                    self.substitute_generics_in_return(&derefed, &raw)
                };
                self.infer_combinator_block(method_name, &block, &ret_ty, &expr.span);
                expr.ty = self.ctx.resolve(&ret_ty);
```

- [ ] **Step 4: Collapse the duplicated harvest onto a shared helper**

The harvest at 526-545 builds `param_names` from `signature.generic_params`, loops args×params calling
`Self::bind_type_params_from_args`, and `Self::subst_ty`s the result into `ret`. Extract that driver
into one place so BOTH the `FnCall` path (collect.rs / the free-fn handler) and `infer_selected_method`
call it. Add to `collect.rs` next to `bind_type_params_from_args` (855):

```rust
    /// Harvest `{generic_param → concrete}` from (args × formal params) and
    /// substitute into `ret`. Single driver shared by the FnCall path and the
    /// selected-method path so the two stay consistent (both already used
    /// bind_type_params_from_args + subst_ty — this removes the duplicated
    /// driver loop in infer/expr.rs MethodCall).
    pub(super) fn harvest_and_subst_generics(
        &self,
        generic_params: &[GenericParam],
        params: &[Param],
        args: &[HirExpr],
        ret: &Ty,
    ) -> Ty {
        if generic_params.is_empty() {
            return ret.clone();
        }
        let names: std::collections::HashSet<String> =
            generic_params.iter().map(|gp| gp.name.clone()).collect();
        let mut bindings = std::collections::HashMap::new();
        for (arg, param) in args.iter().zip(params) {
            let actual = self.ctx.resolve(&arg.ty);
            Self::bind_type_params_from_args(&names, &param.ty, &actual, &mut bindings);
        }
        if bindings.is_empty() { ret.clone() } else { Self::subst_ty(ret, &bindings) }
    }
```

> Implementer: align the exact `GenericParam`/`Param`/`HirExpr` paths with the real signatures (read
> the `signature.generic_params` / `signature.params` types in `expr.rs:526-534`). If the `FnCall`
> handler's harvest differs in any detail (e.g. it resolves args differently), DO NOT force-merge —
> keep the helper matching the `MethodCall` semantics and only route the `MethodCall` caller through it
> this task; note the `FnCall` caller as a follow-up. The goal is one driver for the MethodCall dup
> first; sharing with FnCall is a bonus only if byte-identical.

- [ ] **Step 5: Run to verify green (behaviour unchanged)**

Run: `cargo test -p ruxen_core --test regex_typeck 2>&1 | tee tmp/test-cache/phase6-task3-green.log`
Expected: PASS, identical set to baseline. Then a broader typeck pin:
`cargo test -p ruxen_core --lib typeck 2>&1 | tee tmp/test-cache/phase6-task3-typeck.log`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add compiler/ruxen_core/src/typeck/infer/expr.rs compiler/ruxen_core/src/typeck/infer/collect.rs compiler/ruxen_core/tests/regex_typeck.rs
git commit -m "refactor(typeck): split MethodCall god-arm; dedupe generic harvest

Carve the ~300-line MethodCall inference arm into infer_constructor_call /
infer_selected_method / infer_combinator_block. Replace the inline
generic-harvest driver (which re-implemented collect.rs's
bind_type_params_from_args loop + subst_ty) with one shared
harvest_and_subst_generics. Pure extraction — regex_typeck pins unchanged.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

## Bucket (c) — migrate the remaining hand-rolled walkers onto Phase 1's primitives

### Task 4: Migrate `formatter/comments.rs` span walk onto `parser::visit::Visit`

**Files:**
- Modify: `compiler/ruxen_core/src/formatter/comments.rs` — the nested `fn visit_expr(spans, expr)`
  (690-853) and its sibling `visit_pattern` / block helpers that feed the same `spans` vector.
- Test: `compiler/ruxen_core/tests/formatter_corpus_roundtrip.rs` is the existing pin.

**Why:** `comments.rs` hand-rolls a full `ExprKind` recursion purely to collect every node's `.span`
into `Vec<(usize, usize)>`. That is exactly what `parser::visit::Visit` does, minus the per-node
side-effect. Replacing it removes one drift-prone copy and inherits exhaustiveness.

- [ ] **Step 1: Characterization test**

Confirm `formatter_corpus_roundtrip.rs` exercises a source file containing the span-bearing forms the
old walker recursed into (method calls, closures, match, if/elsif/else, ranges, enum variants). If
coverage is thin, add a corpus fixture that includes a nested `EnumVariant`/`UnsafeBlock`/`MapLiteral`
(the forms most likely to have been missed by a hand-rolled `_ =>`-style walk — verify whether the old
`visit_expr` even has a catch-all). The roundtrip output (comment placement) must be byte-identical.

- [ ] **Step 2: Run baseline**

Run: `cargo test -p ruxen_core --test formatter_corpus_roundtrip 2>&1 | tee tmp/test-cache/phase6-task4-baseline.log`
Expected: PASS.

- [ ] **Step 3: Replace the walk with a `Visit` impl**

Define a span-collector that overrides every node-entry to push the span, then recurses via the shared
`walk_*`:

```rust
use crate::parser::visit::{walk_expr, walk_block, walk_stmt, walk_pattern, walk_type_expr, Visit};

struct SpanCollector<'a> { spans: &'a mut Vec<(usize, usize)> }

impl Visit for SpanCollector<'_> {
    fn visit_expr(&mut self, e: &Expr) { add_span(self.spans, &e.span); walk_expr(self, e); }
    fn visit_block(&mut self, b: &Block) { add_span(self.spans, &b.span); walk_block(self, b); }
    fn visit_stmt(&mut self, s: &Statement) { walk_stmt(self, s); }
    fn visit_pattern(&mut self, p: &Pattern) { add_span(self.spans, p.span_of()); walk_pattern(self, p); }
    // ... type_expr likewise if the old walker recorded TypeExpr spans
}
```

Replace the call sites that invoked the old free `visit_expr(spans, expr)` with
`SpanCollector { spans }.visit_expr(expr)`. Delete the old `visit_expr`/`visit_pattern` free fns.

> Implementer: the old walker recorded `add_span` for `expr.span` AND for pattern spans (the
> `visit_pattern` at ~660-688) AND possibly block spans. Mirror EXACTLY which node kinds it recorded —
> if it did NOT record block spans, drop the `visit_block` add_span override and only recurse. The
> roundtrip test is the backstop for "same spans collected, same order." If Phase 1's `walk_*` visits
> children in a different ORDER than the old walk and the formatter depends on span order, sort or
> preserve order to match — verify against the baseline log.

- [ ] **Step 4: Run green**

Run: `cargo test -p ruxen_core --test formatter_corpus_roundtrip 2>&1 | tee tmp/test-cache/phase6-task4-green.log`
Expected: PASS, identical to baseline.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/formatter/comments.rs
git commit -m "refactor(formatter): comment span-collection uses parser::visit::Visit

Replace the hand-rolled ExprKind recursion in comments.rs with a
SpanCollector Visit impl over Phase 1's exhaustive walk_*. One fewer
drift-prone AST walker; corpus roundtrip pins span output unchanged.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

### Task 5: Migrate the repl `eval.rs` walkers onto `parser::visit`

**Files:**
- Modify: `src/ruxen_repl/src/eval.rs` — the eight hand-rolled walkers:
  `rhs_mutates_session_var` (197-252, inner `walk` 217-250), `replay_region_mutates_slot` (286-447,
  inner `walk_expr` 290 / `walk_block` 380 / `walk_if` 390 / `walk_closure_body` 407 / `walk_stmt`
  414), `collect_mutation_targets` (448-474) + `walk_expr_for_targets` (506-~560),
  `statements_contain_return` (2252-2261), `expr_contains_return` (2269-2381),
  `block_contains_return` (2383-2385).
- Test: `src/ruxen_repl/tests/repl_tests.rs` + `installed_repl.rs` (the replay fixtures 89/90/404/712
  cited in the walker doc-comments are the behavioural pins).

**Why:** Each of these is a "scan the AST for X" recursion with its own `_ => false`/`_ => {}`
catch-all — the exact drift class Phase 1 exists to kill. `expr_contains_return` (2269) even has a
doc-comment admitting its wildcard "must be audited when ExprKind grows." Migrating to `Visit` makes
new variants a compile error and deletes the copies.

> **Note on closure opacity:** three of these walkers (`rhs_mutates_session_var`,
> `expr_contains_return`) DELIBERATELY do not recurse into closure bodies, while
> `replay_region_mutates_slot` DELIBERATELY DOES (it has `walk_closure_body`). This is a real semantic
> difference, not drift. Preserve it per-visitor via the Phase 1 "override `visit_expr`, match
> `Closure` → recurse-or-not, else `walk_expr`" pattern (same technique Phase 1 used for the async
> await-scan). Each migrated visitor's `Closure` arm is load-bearing and MUST carry a comment citing
> the fixture that depends on it.

- [ ] **Step 1: Characterization tests FIRST**

For EACH walker, pin its current behaviour through the repl surface, citing the fixtures named in the
doc-comments:
- `replay_region_mutates_slot`: fixture 90 (`count += 1` in a closure ⇒ refresh) and 89_closure_capture_immut
  (read-only capture ⇒ NO refresh, must not E1009). These are the closure-DOES-recurse cases.
- `rhs_mutates_session_var`: a `let x = v.remove(0)` (RHS mutates receiver ⇒ true) and a `let x = || y`
  (closure RHS ⇒ false, NOT inspected). Closure-does-NOT-recurse case.
- `expr_contains_return`: a `return` inside a `match` arm / `if` body ⇒ true; a `return` inside a
  closure body ⇒ FALSE (closure is its own fn). Closure-does-NOT-recurse case.
- `collect_mutation_targets` / `walk_expr_for_targets`: a control-flow body assigning a session var ⇒
  the var is collected.

If `repl_tests.rs` doesn't already cover each, add a minimal REPL-input test per walker (these are
behaviour pins; they assert the OBSERVABLE replay/return-handling outcome, not the private fn).

- [ ] **Step 2: Run baseline**

Run: `cargo test -p ruxen_repl 2>&1 | tee tmp/test-cache/phase6-task5-baseline.log`
Expected: PASS. (This is the repl crate's narrow suite — it IS the relevant suite for these walkers;
per rule 42 we run the crate that owns the changed code, not the whole workspace.)

- [ ] **Step 3: Migrate each walker to a `Visit` impl**

Replace each free walker's body with a small `Visit` struct. Examples:

```rust
use ruxen_core::parser::visit::{walk_expr, Visit};

// expr_contains_return (2269) — closure bodies are a SEPARATE fn, so the
// Closure arm does NOT recurse (fixture: tail-return handling in build_program).
struct ReturnScan { found: bool }
impl Visit for ReturnScan {
    fn visit_expr(&mut self, e: &Expr) {
        if self.found { return; }
        match &e.kind {
            ExprKind::Return(_) => self.found = true,
            ExprKind::Closure(_) => {}            // own fn — do not recurse
            _ => walk_expr(self, e),
        }
    }
}
fn expr_contains_return(expr: &Expr) -> bool {
    let mut s = ReturnScan { found: false }; s.visit_expr(expr); s.found
}
fn block_contains_return(block: &Block) -> bool {
    let mut s = ReturnScan { found: false }; s.visit_block(block); s.found
}
fn statements_contain_return(stmts: &[Statement]) -> bool {
    let mut s = ReturnScan { found: false };
    for st in stmts { s.visit_stmt(st); }
    s.found
}
```

```rust
// replay_region_mutates_slot (286) — closure bodies DO count (fixture 90),
// so the Closure arm recurses; the "mutates THIS slot name" check stays.
struct SlotMutScan<'a> { name: &'a str, found: bool }
impl Visit for SlotMutScan<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        if self.found { return; }
        match &e.kind {
            ExprKind::Assign { target, .. } | ExprKind::CompoundAssign { target, .. }
                if matches!(&target.kind, ExprKind::Identifier(n) if n == self.name) =>
                { self.found = true; }
            _ => walk_expr(self, e),   // Closure recurses via the default arm — fixture 90
        }
    }
}
```

> Implementer obligations:
> - `rhs_mutates_session_var` keeps its `receiver_is_session_var` helper; the outer `walk` becomes a
>   `Visit` whose `MethodCall`/`SafeNavCall` arm runs the receiver check then `walk_expr`s, and whose
>   `Closure` arm does NOT recurse (matches the current "closures NOT inspected" doc-comment at 195).
> - `collect_mutation_targets` + `walk_expr_for_targets` become one `Visit` that, on each
>   assign/methodcall/fieldaccess/closurecall node, runs `base_identifier` into the shared `names`
>   set, then recurses. `base_identifier` stays as a free helper.
> - DELETE every inner `fn walk*` once its `Visit` replacement compiles. After this task, `grep -nE
>   'fn walk' src/ruxen_repl/src/eval.rs` should return ZERO ad-hoc tree walkers (only the trait-method
>   `visit_*` overrides remain).

- [ ] **Step 4: Run green (behaviour unchanged)**

Run: `cargo test -p ruxen_repl 2>&1 | tee tmp/test-cache/phase6-task5-green.log`
Expected: PASS, identical to baseline. Diff:
`diff <(grep -E "test result|FAILED" tmp/test-cache/phase6-task5-baseline.log) <(grep -E "test result|FAILED" tmp/test-cache/phase6-task5-green.log)`
Expected: no differences. Pay special attention to fixtures 89/90/404/712 (the closure-opacity pins).

- [ ] **Step 5: Commit**

```bash
git add src/ruxen_repl/src/eval.rs src/ruxen_repl/tests/repl_tests.rs
git commit -m "refactor(repl): migrate 8 eval.rs walkers onto parser::visit::Visit

rhs_mutates_session_var, replay_region_mutates_slot, collect_mutation_targets,
walk_expr_for_targets, statements/expr/block_contains_return now use the
shared exhaustive Visit. Per-visitor closure opacity preserved (return-scan
and rhs-mutate do NOT recurse into closures; slot-mutate DOES — fixtures
89/90/404/712 pin this). Removes the last _ => false/_ => {} AST catch-alls
in the repl.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

### Task 6: Consolidate `monomorphize.rs` `walk_tys_in_*` behind one shared HIR-Ty visitor

**Files:**
- Modify: `compiler/ruxen_core/src/mir/lower/monomorphize.rs` —
  `walk_tys_in_{program,item,func,expr,stmt}` (321-~530).
- Test: the mono pins (`grep` the tests dir for `monomorph`/`mono` — e.g.
  `compiler/ruxen_core/tests/` class-mono coverage; `drop_fixtures.rs` indirectly exercises mono'd
  output). Add a focused unit test in `monomorphize.rs`'s test module if one exists.

**Why (and the IMPORTANT caveat):** These five fns walk the **HIR** expr/item tree calling `f(&Ty)` on
every reachable `Ty`. They are NOT a `Ty` fold (so `Ty::map_inner` does not directly apply — that folds
WITHIN one `Ty`, whereas these walk the HIR tree to FIND the `Ty`s), and they walk `HirExprKind`, NOT
the parser `ExprKind` (so Phase 1's `parser::visit::Visit` does not apply either — different enum).
`walk_tys_in_item` (370) and `walk_tys_in_expr` (504) each carry a `_ => {}` catch-all — the drift
hazard to remove. The realistic migration is therefore:

1. **Make the two `_ => {}` arms exhaustive** — enumerate the remaining `HirItem` / `HirExprKind`
   variants explicitly so adding a variant is a compile error. This is the concrete drift fix.
2. **Compose each found `Ty` through `Ty::map_inner`** where the walker currently calls `f(&ty)` on a
   composite type and relied on the caller to recurse into nested `Ty` children — verify whether the
   callers need the NESTED `Ty`s visited (e.g. the `Box[String]` inside an `Array[Box[String]]`). If
   the existing behaviour only visits top-level `.ty` values and the mono pass separately re-derives
   nested ones, do NOT change that; just close the catch-alls.

> **Implementer decision sub-step (explicit):** before writing code, determine whether a shared HIR
> visitor is warranted or whether this task is *only* the two-catch-all-removal. If `ruxen_core` has NO
> other HIR-tree walker (grep `fn .*HirExpr` for recursive walkers), then YAGNI says: do NOT invent a
> `HirVisit` trait for a single consumer — just make the two matches exhaustive. If a second HIR walker
> exists (e.g. in drop-elaboration or another mono pass) and is byte-similar, THEN extract a shared
> `walk_hir_tys(program, &mut f)` free fn. Record the decision in the commit message. The master plan's
> "migrate to Ty::map_inner-style or a shared Ty visitor" explicitly allows either; pick by caller
> count, not by aspiration.

- [ ] **Step 1: Characterization test FIRST**

Pin that the mono pass still specializes a generic class/fn that appears only inside the
currently-`_ => {}`-swallowed HIR forms. Construct (or find) a program where a `Box[String]`-style
monomorphization target is reachable ONLY through an expr kind the current `_ => {}` drops (audit which
`HirExprKind` variants are NOT explicitly matched in 384-504 — e.g. `SafeNav`, `MethodCall`'s
`generic_args`, `Try`, `Await` if present in HIR). If such a hole exists, that is a latent bug: write
the test to EXPOSE it (red), and closing the catch-all fixes it. If no hole exists (every reachable
target is already matched), write the test as a pure characterization pin (green throughout).

- [ ] **Step 2: Run baseline / red**

Run: `cargo test -p ruxen_core --lib mir::lower::monomorphize 2>&1 | tee tmp/test-cache/phase6-task6-red.log`
(plus the integration mono pin if one exists). Expected: characterization cases PASS; any
catch-all-hole case FAILS.

- [ ] **Step 3: Close the catch-alls (and optionally extract the shared walker)**

In `walk_tys_in_item` (370) and `walk_tys_in_expr` (504), replace `_ => {}` with explicit arms for
every remaining variant (those with no nested `Ty`/`HirExpr` get an empty `=> {}` body, but named, so
the compiler enforces exhaustiveness). For any variant that DOES carry a reachable `Ty` or child
`HirExpr` the old code dropped, recurse into it. If the implementer-decision sub-step chose extraction,
move the five fns' bodies behind `walk_hir_tys`.

> Keep `f: &mut impl FnMut(&Ty)` exactly. If a composite `Ty` is visited and the caller needs its
> nested children too, the call site can wrap `f` to also `ty.map_inner(&mut |c| { f(c); c.clone() })`
> — but only add that if Step 1 proved a nested-Ty target was being missed. Do not add it speculatively.

- [ ] **Step 4: Run green**

Run: `cargo test -p ruxen_core --lib mir::lower::monomorphize 2>&1 | tee tmp/test-cache/phase6-task6-green.log`
Expected: PASS including the (formerly red) catch-all-hole case if one existed.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/mir/lower/monomorphize.rs
git commit -m "refactor(mir): make walk_tys_in_* exhaustive (close _ => {} drift)

Replace the two _ => {} catch-alls in walk_tys_in_item/expr with explicit
per-variant arms so a new HirItem/HirExprKind variant is a compile error
instead of a silently-skipped monomorphization target. [Extracted shared
walk_hir_tys | kept five fns — single consumer, no abstraction warranted.]

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

## Bucket (d) — cheap wins

### Task 7: One shared `COLLECTION_BUILTINS` const

**Files:**
- Modify: `compiler/ruxen_core/src/resolve/types.rs` — add the const; `resolve_type_expr` (18; the
  collection arms at 409 / 417 / 439).
- Modify: `compiler/ruxen_core/src/resolve/ffi_registration.rs` — the `is_anchor_only_builtin`
  `matches!` at 494-497.
- Test: `compiler/ruxen_core/tests/stdlib_bootstrap.rs` / any collection-typing pin (e.g. a test that
  `let x: Array[Int]` resolves to `Ty::Array`, and that the ffi anchor path treats `Map` as
  anchor-only).

**Why:** The literal set `"Array"|"Vec"|"Map"|"HashMap"|"Set"|"HashSet"` is written twice: once as a
flat `matches!` in `ffi_registration.rs:494` (whose comment literally says "The list mirrors the arms
in `resolve_type_expr` lines ~4605-4659 1:1"), and once as three separate behaviour-bearing arms in
`resolve_type_expr` (409/417/439). The two can drift; pin them to one const.

> **IMPORTANT scope limit (read before coding):** `resolve_type_expr`'s three arms each produce a
> DIFFERENT `Ty` (`Array`/`Map`/`Set`) and carry distinct logic (Map/Set hash-key validation, E0615).
> They CANNOT collapse into one membership check without changing behaviour. So this task ONLY:
> (1) defines `pub(crate) const COLLECTION_BUILTINS: &[&str] = &["Array","Vec","Map","HashMap","Set","HashSet"];`
> in `resolve/types.rs`; (2) rewrites `ffi_registration.rs`'s `is_anchor_only_builtin` to
> `COLLECTION_BUILTINS.contains(&class.name.as_str())`; (3) updates the `ffi_registration.rs` comment
> to reference the const as the single source of truth instead of the (now-wrong) "~4605-4659" line
> ref. Leave the three `resolve_type_expr` arms structurally as-is — but the implementer MAY add a
> debug-assert or a doc-comment cross-link so a future editor knows the const must list exactly the
> names those three arms handle.

- [ ] **Step 1: Characterization test**

Add/confirm a test asserting both consumers agree on the set: `let x: Map[Int, Int]` resolves to
`Ty::Map` (resolve_type_expr arm), AND a `class Map`-shaped anchor goes through `is_anchor_only_builtin
== true` (ffi path). A single test naming all six builtins and asserting both predicates is fine.

- [ ] **Step 2: Run baseline**

Run: `cargo test -p ruxen_core --test stdlib_bootstrap 2>&1 | tee tmp/test-cache/phase6-task7-baseline.log`
Expected: PASS.

- [ ] **Step 3: Define the const and route ffi through it**

Add the const to `resolve/types.rs`. In `ffi_registration.rs`:

```rust
use crate::resolve::types::COLLECTION_BUILTINS;
// ...
let is_anchor_only_builtin = COLLECTION_BUILTINS.contains(&class.name.as_str());
```

Update the stale comment block (491-493) to cite `COLLECTION_BUILTINS` as the single source of truth.

- [ ] **Step 4: Run green**

Run: `cargo test -p ruxen_core --test stdlib_bootstrap 2>&1 | tee tmp/test-cache/phase6-task7-green.log`
Expected: PASS, unchanged.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/resolve/types.rs compiler/ruxen_core/src/resolve/ffi_registration.rs
git commit -m "refactor(resolve): share COLLECTION_BUILTINS const across resolve/ffi

The Array|Vec|Map|HashMap|Set|HashSet literal set was duplicated in
ffi_registration.rs (matches!) and resolve_type_expr (a stale comment even
pointed at wrong line numbers). Define one pub(crate) COLLECTION_BUILTINS;
ffi anchor-check now reads it. resolve_type_expr's three Ty-producing arms
keep their distinct bodies (different output + hash-key validation).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

### Task 8: Rename `library/std/foobar` → `library/std/_pin_zero_rust_stdlib`

**Files:**
- Rename: `library/std/foobar/` → `library/std/_pin_zero_rust_stdlib/` (incl. `Ruxen.toml`,
  `src/lib.rx`, `runtime/foobar.c`).
- Modify: `library/std/_pin_zero_rust_stdlib/Ruxen.toml` (`name = "std-foobar"` → new name).
- Modify: `compiler/ruxen_core/src/resolve/bootstrap.rs:160` (`"foobar/src/lib.rx"` entry).
- Modify: `compiler/ruxen_core/src/resolve/stdlib_embedded.rs:127-128` (the `"foobar/src/lib.rx"` table
  key AND the `include_str!("../../../../library/std/foobar/src/lib.rx")` path).
- Modify: `compiler/ruxen_core/tests/trio_leak_pin.rs` (package name, the `use std.foobar` source
  strings, the lowercase `"foobar"` substring scan, the doc-comments).
- Test: `compiler/ruxen_core/tests/trio_leak_pin.rs` is itself the pin.

**Why:** `foobar` is NOT junk — it is the documented B5 trio-leak pin fixture
(`docs/specs/system/zero_rust_stdlib_classes.spec.md`). The rename makes its purpose self-evident so no
future contributor deletes it as cruft.

> **CRITICAL behaviour caveat (the rename is NOT free):** `bootstrap_package_names()`
> (bootstrap.rs:200) derives the std-namespace segment from the path's FIRST segment —
> `foobar/src/lib.rx` ⇒ `std.foobar`. The test `foobar_resolves_through_std_namespace`
> (trio_leak_pin.rs:143) compiles `use std.foobar.FooBar`. After the rename the namespace becomes
> `std._pin_zero_rust_stdlib`, so that `use` MUST be updated too. ALSO verify the lexer/parser accept a
> module-path segment beginning with `_` and containing `_` (identifier rules). If `_pin_zero_rust_stdlib`
> is NOT a legal path segment, FALL BACK to a legal name like `pin_zero_rust_stdlib` (no leading
> underscore) and note it — do not ship a name the resolver can't import. Decide this in Step 1.

- [ ] **Step 1: Verify the target name is a legal namespace segment**

Run a tiny probe (or read the lexer ident rules): does `use std._pin_zero_rust_stdlib.FooBar` tokenize
and resolve as a path? If yes, proceed with `_pin_zero_rust_stdlib`. If no, use `pin_zero_rust_stdlib`.
Record the chosen name; use it consistently below. (The directory name and the path-first-segment
namespace MUST match, because `bootstrap_package_names` ties them together.)

- [ ] **Step 2: Run the pin at baseline**

Run: `cargo test -p ruxen_core --test trio_leak_pin 2>&1 | tee tmp/test-cache/phase6-task8-baseline.log`
Expected: PASS (or whatever the current `#[ignore]` state is — note it; the master plan flags
`#[ignore]` is banned, so if `foobar_addition_touches_only_bootstrap_files` is still `#[ignore]`d,
un-ignoring it is a separate concern — DO NOT silently keep it ignored, surface it).

- [ ] **Step 3: Rename the directory and update all references**

```bash
git mv library/std/foobar library/std/_pin_zero_rust_stdlib
```

Then edit (using the dedicated Edit tool, not echo):
- `library/std/_pin_zero_rust_stdlib/Ruxen.toml`: `name = "std-foobar"` → `name = "std-_pin_zero_rust_stdlib"`
  (match the resolver's package-name expectation; if the resolver derives package id from the toml
  `name` anywhere, keep it consistent with the directory).
- `compiler/ruxen_core/src/resolve/bootstrap.rs:160`: `"foobar/src/lib.rx"` →
  `"_pin_zero_rust_stdlib/src/lib.rx"` (and update the surrounding comment at 155-159).
- `compiler/ruxen_core/src/resolve/stdlib_embedded.rs:127-128`: both the table key and the
  `include_str!` path → `_pin_zero_rust_stdlib/...`.
- `compiler/ruxen_core/tests/trio_leak_pin.rs`: the `use std.foobar.FooBar` source (143+), the
  `PERMITTED`/offender substring scan (it greps lowercase `"foobar"` — change to the new name's lower
  form, AND verify the embedded `lib.rx` doc-comments inside the fixture no longer say "foobar" or the
  leak-scan will flag the embedded `include_str!`'d content — update `src/lib.rx`'s header comment
  too), and the doc-comments naming the fixture.

> Implementer: the leak-scan at trio_leak_pin.rs:217 does `contents.to_lowercase().contains("foobar")`
> over every `compiler/ruxen_core/src/*.rs`. `stdlib_embedded.rs` `include_str!`s the fixture's
> `lib.rx` at COMPILE time but the scan reads the `.rs` SOURCE (which only contains the path string,
> now renamed) — so the scan target is the renamed path, fine. But double-check no other `.rs` file
> mentions "foobar"; `grep -rn foobar compiler/ruxen_core/src` must be empty after the rename (except
> permitted files, now renamed). Also rename `runtime/foobar.c` if any `lib "runtime/foobar.c"` decl in
> `src/lib.rx` references it — keep the decl and the file name in sync.

- [ ] **Step 4: Run the pin to prove the bootstrap still loads the fixture**

Run: `cargo test -p ruxen_core --test trio_leak_pin --test stdlib_bootstrap 2>&1 | tee tmp/test-cache/phase6-task8-green.log`
Expected: PASS — the renamed package bootstraps, `std.<newname>.FooBar` resolves, Send/Sync auto-derive
holds, and the leak-scan passes with the new name. Also run the embedded-sync pin:
`cargo test -p ruxen_core --lib resolve::stdlib_embedded 2>&1 | tee tmp/test-cache/phase6-task8-embed.log`
Expected: PASS (`BOOTSTRAP_FILES` ↔ `BOOTSTRAP_EMBEDDED` still in lockstep).

- [ ] **Step 5: Commit**

```bash
git add -A library/std/_pin_zero_rust_stdlib compiler/ruxen_core/src/resolve/bootstrap.rs compiler/ruxen_core/src/resolve/stdlib_embedded.rs compiler/ruxen_core/tests/trio_leak_pin.rs
git commit -m "refactor(stdlib): rename foobar pin fixture to _pin_zero_rust_stdlib

The foobar package is the documented B5 trio-leak pin fixture, not junk.
Rename it to a self-describing name so it is not mistaken for cruft.
Updates BOOTSTRAP_FILES, the embedded-stdlib include table, the Ruxen.toml
name, and the trio_leak_pin assertions (incl. the std-namespace import and
the leak substring scan). Bootstrap pin proves the fixture still loads.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

### Task 9: Finalize the `ruxen_noop_passthrough` decision

**Files:**
- Read-only audit: `compiler/ruxen_core/src/codegen/lang_intrinsics.rs` (64/66/167/186),
  `codegen/llvm/emit/instructions.rs:167`, `codegen/runtime/tests_resolve.rs` (10/63/66),
  `compiler/ruxen_core/tests/codegen_unknown_method_rejected.rs` (3-6).
- Write: a short decision record (append to this plan's Self-Review, or to the relevant spec if one
  exists). NO compiler code change expected.

**Why:** The master plan's risk register flags `ruxen_noop_passthrough` as possibly the global-rule-
banned `ruxen_noop_passthrough`/dead-code-bypass. The audit shows it is NOT: it is a legitimate codegen
intrinsic *alias* — `"yield"` and `"&str_as_str"` legitimately lower to a no-op passthrough symbol
(lang_intrinsics.rs:64/66), and `codegen_unknown_method_rejected.rs` PROVES the BANNED behaviour (using
`ruxen_noop_passthrough` as a catch-all for any unrecognised `?T_xxx_method` mangled name) was already
REMOVED — unrecognised methods now error (tests_resolve.rs:10 "P0.5: it must error instead").

- [ ] **Step 1: Confirm the audit holds at current HEAD**

Run: `cargo test -p ruxen_core --test codegen_unknown_method_rejected 2>&1 | tee tmp/test-cache/phase6-task9-audit.log`
Expected: PASS — unknown methods are rejected, not passed through. Also confirm
`cargo test -p ruxen_core --lib codegen::runtime::tests_resolve` passes (the `yield` /
`&str_as_str` → `ruxen_noop_passthrough` aliases are intentional).

- [ ] **Step 2: Record the decision**

Append to this plan's Self-Review section (below) the one-paragraph decision: `ruxen_noop_passthrough`
is a KEPT, legitimate intrinsic alias for genuine no-op-lowered language constructs (`yield`,
`&str_as_str`); it is NOT the banned unknown-method bypass — that bypass was removed in P0.5 and is
pinned removed by `codegen_unknown_method_rejected.rs`. No further action. If Phase 4 already recorded
this, cite Phase 4 and close.

- [ ] **Step 3: Commit (docs-only, if any doc was changed)**

If a spec/doc was touched:
```bash
git add docs/superpowers/plans/2026-06-04-phase6-cleanup-sweep.md
git commit -m "docs(phase6): record ruxen_noop_passthrough kept-intrinsic decision

ruxen_noop_passthrough is a legitimate intrinsic alias (yield, &str_as_str),
not the banned unknown-method bypass — that bypass was removed in P0.5 and
is pinned removed by codegen_unknown_method_rejected. No code change.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff) — for a docs-only commit, confirm no code drift
  and that the decision is recorded.

---

## Task 10: Phase-6 final integration

**Files:** none (verification only).

- [ ] **Step 1: Run the full owning-crate suites once**

Run: `cargo test -p ruxen_core 2>&1 | tee tmp/test-cache/phase6-final-core.log`
Run: `cargo test -p ruxen_repl 2>&1 | tee tmp/test-cache/phase6-final-repl.log`
Expected: all green. Per rules 41/42 these are the ONLY full-crate runs in Phase 6; intermediate tasks
ran only their narrow tests.

- [ ] **Step 1b: Full multi-agent `/thermonuke` sweep (phase gate)**

Invoke the `thermonuke` skill on the whole Phase 6 diff (`git diff <phase6-base>..HEAD`). It must
confirm: the `derive.rs` inline debug ladder is DELETED and five field-walks share `fold_struct_fields`;
the `MethodCall` arm is split and the harvest dup is gone; the repl's 8 walkers + comments.rs walk are
REPLACED (not added alongside) by `parser::visit`; `monomorphize`'s two `_ => {}` arms are closed;
`COLLECTION_BUILTINS` exists once; the pin fixture is renamed and still loads; net lines reduced. Surface
the report to the maintainer.

- [ ] **Step 2: Confirm no new catch-all sneaked in and the walkers are gone**

Run: `grep -nE '_ =>' compiler/ruxen_core/src/mir/lower/monomorphize.rs | grep -vE 'mod tests'`
Expected: NO `_ =>` in `walk_tys_in_*`.
Run: `grep -nE 'fn walk' src/ruxen_repl/src/eval.rs`
Expected: ZERO ad-hoc tree walkers remain (only `Visit` trait `visit_*` overrides).
Run: `grep -rn foobar compiler/ruxen_core/src`
Expected: empty (the rename is complete).

- [ ] **Step 3: Report**

Report to maintainer: net line delta (`git diff --stat <phase6-base>..HEAD`), the four buckets'
outcomes, both full-crate suites green (cite the two final logs), and the statement: "No behaviour
changed except the intentional struct-debug nested-enum rendering improvement (Task 1); every other
migration is pinned by a characterization test." Await go-ahead to close the phase.

---

## Self-Review (run before handing off)

**Spec coverage:** bucket (a) Tasks 1-2 (derive dedup + `fold_struct_fields`); bucket (b) Task 3
(`MethodCall` split + harvest dedup); bucket (c) Tasks 4-6 (comments / repl×8 / monomorphize); bucket
(d) Tasks 7-9 (`COLLECTION_BUILTINS` / foobar rename / noop decision). All four master-plan buckets are
covered.

**Phase-1 dependency honoured:** Tasks 4-5 consume `parser::visit::{Visit, walk_expr, walk_block,
walk_stmt}`; Task 6 references `Ty::map_inner` but explicitly documents WHY it largely does not apply
(HIR-tree walk finding `Ty`s, not a within-`Ty` fold; `HirExprKind` ≠ parser `ExprKind`) and falls back
to closing the catch-alls — the honest, non-aspirational choice. Task 3's harvest reuses
`collect.rs::bind_type_params_from_args` (Phase-1-adjacent, already present).

**Behaviour-change ledger:** exactly ONE intentional behaviour change — Task 1 renders a nested
`derive Debug` enum field via `{enum}_to_debug` instead of `<...>` (the bypassed helper was more
complete than the inline copy). Every other task is behaviour-preserving, pinned by a characterization
test run green before and after (logs diffed). Task 6 may close a latent monomorphization hole IF the
audit finds one; if so it is a bug FIX exposed red-first, not a silent change.

**YAGNI guards applied:** `fold_struct_fields` is introduced only because it has 4-5 real callers (Task
2 explicitly refuses to contort it for `default` if `default` doesn't fit). Task 6 explicitly refuses to
invent a `HirVisit` trait for a single consumer — it closes catch-alls and extracts only if a second
HIR walker already exists. Task 7 explicitly refuses to collapse `resolve_type_expr`'s three distinct
`Ty`-producing arms (different output + E0615 validation) into one membership check.

**Placeholder scan:** no `todo!()`/dead-code stubs. The two places needing implementer judgement (Task
6's "shared walker vs catch-all-only" decision; Task 8's "is `_pin_zero_rust_stdlib` a legal namespace
segment" probe) are explicit decision sub-steps with a stated fallback, not hidden placeholders.

**`ruxen_noop_passthrough` decision (Task 9):** KEPT. It is a legitimate codegen intrinsic alias for
no-op-lowered language constructs (`yield`, `&str_as_str` in `lang_intrinsics.rs:64/66`). It is NOT the
global-rule-banned unknown-method bypass — that bypass was removed in P0.5 and is pinned removed by
`compiler/ruxen_core/tests/codegen_unknown_method_rejected.rs`. No code change; recorded here per the
master-plan risk register.

**Test-cache discipline:** every task tees its narrow run to `tmp/test-cache/phase6-taskN-{red,baseline,green,...}.log`;
the full `cargo test -p ruxen_core` and `-p ruxen_repl` run ONCE in Task 10 (per rules 41/42).

**Est. net reduction:** bucket (a) ~−250, bucket (b) ~−270, bucket (c) ~−500+, bucket (d) ~−20 + a
rename. Aggregate ~−1,000+ lines, matching the master plan's Phase 6 target.
