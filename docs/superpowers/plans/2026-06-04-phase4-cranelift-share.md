# Phase 4 — Share the Cranelift core (`TranslationEnv<M: Module>`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`)
> syntax for tracking. This is Phase 4 of `2026-06-04-thermonuke-master.md`. **HIGHEST-RISK PHASE —
> spike-gated.** Task 0 is a MANDATORY de-risking spike with an explicit go/no-go; do NOT start Task 1
> until Task 0 has decided between the generic path and the macro fallback.

**Goal:** Delete the ~1,973-line near-verbatim Cranelift FORK in `src/ruxen_repl/src/jit.rs`. The batch
backend (`compiler/ruxen_core/src/codegen/cranelift/`) already factors its lowering into free functions
(`emit.rs`, `helpers.rs`) parameterised over a borrow-split `TranslationEnv<'a>` — but that struct hard-codes
`module: &'a mut ObjectModule`, so it cannot drive a `JITModule`. jit.rs copies all of it. Genericize
`TranslationEnv` over `cranelift_module::Module`, promote the 12 shared emit/translate helpers from
`pub(super)` to `pub`, and reduce jit.rs to a ~400-line JIT adapter: `JITModule` construction, the
capture-shim `register_runtime_symbols` + `is_repl_symbol_allowed`, and `finalize_definitions` /
`get_finalized_function`. **Est. ~1,500 net deletion.**

**Architecture:** The borrow-split is the crux. `compile_function` constructs a `FunctionBuilder` that
mutably borrows `ctx.func`, AND a `TranslationEnv` that mutably borrows `module`/`declared_fns`/… — two
disjoint `&mut` into `self`. Today both backends do this with a *concrete* module type. The share replaces
`&'a mut ObjectModule` with `&'a mut M` where `M: cranelift_module::Module`. The 6 of 12 helpers that never
touch the module (`coerce_value`, `coerce_value_signed`, `emit_binop`, `cmpop_to_intcc`, `cmpop_to_floatcc`,
`ty_to_cranelift`, `is_string_typed_value`) are already module-agnostic — they only take `&mut FunctionBuilder`
— so they need **no** generic param; promoting their visibility is enough. The remaining helpers
(`translate_instruction`, `translate_terminator`, `gen_value`, `coerce_call_args`, `def_local`) take
`env: &mut TranslationEnv<M>` and become generic over `M` transitively. `build_signature` takes `&M` directly.

**The risk (honest):** `JITModule::declare_func_in_func(func_id, builder.func)` is called from inside
`TranslationEnv::get_or_declare_func` while a `FunctionBuilder` *also* holds `&mut ctx.func`. Both backends
compile today, so the borrow split provably works for two concrete types. The open question Task 0 settles is
whether the *generic* form (`&'a mut M` with `M::declare_func_in_func`) type-checks without `M` being forced
into an over-constrained bound or a lifetime that fights the `builder.func` borrow. The master plan's risk
register flags this as Med likelihood. **Fallback if it fights:** a `macro_rules!` that expands the shared
core into each backend's concrete module type (one source of truth, two monomorphic expansions, zero generic
lifetime puzzle). Task 0 decides; both paths are specified below.

**Tech Stack:** Rust 1.91, `cargo test -p ruxen_repl` / `cargo test -p ruxen_core`, `cranelift_module::Module`,
`cranelift_object::ObjectModule`, `cranelift_jit::JITModule`. **No new dependencies.**

> **Per-task direction-check (maintainer-mandated):** after the commit step of EVERY task below, run the
> `thermonuke` skill scoped to that task's diff (arg `git diff HEAD~1..HEAD`) and confirm (a) net lines moved
> down (forked code DELETED from jit.rs, not duplicated), (b) no new `_ =>` catch-all in any traversal/table,
> (c) no second copy of a helper introduced "temporarily", (d) the task's structural goal was met (a forked
> fn is *gone* from jit.rs and the batch backend's copy is the only one). If it flags drift, STOP and surface
> it. The full multi-agent sweep runs in the final-integration task. Each task's checkbox list ends with a
> `- [ ] Direction-check (/thermonuke on this task's diff)` step.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `compiler/ruxen_core/src/codegen/cranelift/translation_env.rs` | `TranslationEnv` borrow-split + module methods | **Genericize** over `M: Module` |
| `compiler/ruxen_core/src/codegen/cranelift/emit.rs` | 12 emit/translate helpers (currently `pub(super)`) | Promote to `pub`, thread `M` where needed |
| `compiler/ruxen_core/src/codegen/cranelift/helpers.rs` | `ty_to_cranelift`/`cmpop_*`/`is_string_*`/`simple_type_size` | Promote to `pub` (module-agnostic, no `M`) |
| `compiler/ruxen_core/src/codegen/cranelift/mod.rs` | `CodeGen` (ObjectModule) — the batch caller | Modify `build_signature`/`TranslationEnv` call sites to the generic forms |
| `src/ruxen_repl/src/jit.rs` | **forked** Cranelift backend (1,973 lines) | **Reduce to ~400-line JIT adapter** (delete all 12 forked fns + `JITTranslationEnv`) |
| `compiler/ruxen_core/tests/cranelift_share_pin.rs` | both-backends-identical pin test on a fixed MIR fixture | **Create** |
| `docs/decisions/phase4-ruxen-noop-passthrough.md` | record the noop-passthrough ban resolution | **Create** |

**Module placement rationale:** the shared core stays in `codegen/cranelift/` (it owns the MIR→CLIF lowering
obligation); jit.rs becomes a *consumer* of it, importing the now-`pub` helpers. The pin test lives in
`ruxen_core/tests/` because it must construct both a `CodeGen` (ObjectModule) and a `JITCodeGen` (JITModule)
path over one MIR fixture — it depends on both crates' public surface.

---

## Task 0: MANDATORY de-risking spike — generic borrow-split on ONE helper

**This task gates the entire phase. It produces a throwaway commit (reverted at the end of the task), a
go/no-go decision, and selects Path A (generic) or Path B (macro) for Tasks 1–5.**

**Files (throwaway):**
- Spike-modify: `compiler/ruxen_core/src/codegen/cranelift/translation_env.rs`,
  `compiler/ruxen_core/src/codegen/cranelift/emit.rs` (one helper only).

**What we are proving:** that `TranslationEnv<'a, M: cranelift_module::Module>` with
`module: &'a mut M` compiles when `get_or_declare_func` calls
`self.module.declare_func_in_func(func_id, builder.func)` *while* the caller holds `&mut builder` (which
owns `&mut ctx.func`). This is the exact `&mut M` ⊥ `&mut ctx.func` disjointness the generic form must not
break. We prove it on the SMALLEST helper that touches the module: `def_local` does NOT (skip it);
`get_or_declare_func` + `create_string_data` DO. Use `create_string_data` (it touches `module` but takes no
`FunctionBuilder`, so it isolates the `&'a mut M` half) THEN `get_or_declare_func` (it touches both
`module` AND `builder.func`, the real collision). Both must compile for a GO.

- [ ] **Step 1: Create a throwaway spike branch state**

```bash
git checkout -b phase4-spike   # disposable; reverted at task end
```

- [ ] **Step 2: Genericize `TranslationEnv` and the two module-touching env methods (spike scope only)**

In `translation_env.rs`, change the struct head and `impl` to (current concrete form at lines 19–37):

```rust
use cranelift_module::Module;          // already imported

pub(super) struct TranslationEnv<'a, M: Module> {
    pub(super) module: &'a mut M,
    pub(super) declared_fns: &'a mut HashMap<String, FuncId>,
    pub(super) string_data: &'a mut HashMap<String, cranelift_module::DataId>,
    pub(super) string_counter: &'a mut u32,
    pub(super) user_fn_param_tys: &'a HashMap<String, Vec<Type>>,
    pub(super) vtable_data: &'a HashMap<String, cranelift_module::DataId>,
}

impl<'a, M: Module> TranslationEnv<'a, M> {
    // create_string_data: body UNCHANGED — it only uses self.module.declare_data /
    //   define_data, both on the `Module` trait. (lines 39–66)
    // get_or_declare_func: body UNCHANGED — declare_func_in_func / declare_function /
    //   isa() / declare_runtime_func are all `Module` trait methods. (lines 70–169)
    // declare_runtime_func: body UNCHANGED. (lines 172–201)
}
```

> Drop the `use cranelift_object::ObjectModule;` line (13) for the spike — if anything still names
> `ObjectModule` concretely, that's a finding to record.

- [ ] **Step 3: Genericize ONE downstream helper to thread `M` through (spike scope only)**

In `emit.rs`, change `get_or_declare_func`'s nearest *caller* among the 12 — the `translate_instruction`
`Call` arm calls `env.get_or_declare_func(...)`. For the spike, change just the signature of
`translate_instruction` (line 97) and its `build_signature` neighbour to the generic form:

```rust
pub(super) fn translate_instruction<M: cranelift_module::Module>(
    inst: &MirInst,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    _block_map: &[cranelift_codegen::ir::Block],
    builder: &mut FunctionBuilder,
    env: &mut TranslationEnv<M>,
) -> Result<(), String> { /* body UNCHANGED */ }
```

And `build_signature` (line 29): `pub(super) fn build_signature<M: Module>(module: &M, func: &MirFunction)`.

- [ ] **Step 4: Compile the batch backend against the generic form**

The batch `mod.rs` `compile_function` (line 355) already constructs `TranslationEnv { module: &mut self.module, .. }`
where `self.module: ObjectModule`. With the generic struct, `M` infers to `ObjectModule`. Build:

Run: `cargo build -p ruxen_core 2>&1 | tee tmp/test-cache/phase4-task0-spike-build.log`

- [ ] **Step 5: GO / NO-GO decision**

**GO (Path A — generic):** `cargo build -p ruxen_core` succeeds with the generic `TranslationEnv<'a, M: Module>`,
i.e. `self.module.declare_func_in_func(func_id, builder.func)` type-checks with `module: &'a mut M` while
`builder: &mut FunctionBuilder<'_>` is live, and NO `ObjectModule`-concrete name remains required in the shared
core. → Tasks 1–5 use the generic path.

**NO-GO (Path B — macro fallback):** the build fails with a borrow-checker error tying `&'a mut M` to the
`builder.func` borrow (e.g. "cannot borrow `*self.module` as mutable because it is also borrowed"), OR `M`
must be constrained beyond `Module` (an associated-type or `where Self: 'a` that the concrete types don't
satisfy), OR the generic lifetime forces an unworkable signature on the 5 downstream helpers. → Tasks 1–5
use Path B (below).

Record the decision verbatim in the task commit message and in
`docs/decisions/phase4-ruxen-noop-passthrough.md`'s sibling note (or a one-paragraph `phase4-spike-result.md`).

- [ ] **Step 6: Revert the spike, keep only the decision**

```bash
git checkout codegen-bug          # back to the real branch
git branch -D phase4-spike        # discard the throwaway code
```

The spike commits nothing to the working branch except (Step 7) the recorded decision.

- [ ] **Step 7: Commit the decision record only**

```bash
git add docs/decisions/phase4-spike-result.md
git commit -m "chore(codegen): record Phase 4 TranslationEnv<M> spike result (GO/NO-GO)

Spike proved/disproved that TranslationEnv genericized over
cranelift_module::Module borrow-splits cleanly against the FunctionBuilder
borrow of ctx.func. Decision: Path A (generic) | Path B (macro). Tasks 1-5
follow the selected path.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

### Path B — macro fallback (used by Tasks 1–5 ONLY if Task 0 is NO-GO)

If Task 0 says NO-GO, the shared core is expressed as a `macro_rules! impl_cranelift_core` that takes the
concrete module type as a token and expands the env-method bodies + the 5 module-touching helpers into both
backends. The 6 module-agnostic helpers (`coerce_value`, `coerce_value_signed`, `emit_binop`, `cmpop_to_intcc`,
`cmpop_to_floatcc`, `ty_to_cranelift`, `is_string_typed_value`) are STILL shared as ordinary `pub` free
functions in `helpers.rs`/`emit.rs` — they have no `M`, so the macro is unnecessary for them and Tasks 2/2b
proceed identically under either path. The macro covers only `create_string_data` / `get_or_declare_func` /
`declare_runtime_func` / `build_signature` / the env-typed bodies of `translate_instruction` /
`translate_terminator` / `gen_value` / `coerce_call_args`. The macro lives in `translation_env.rs`; jit.rs
invokes `impl_cranelift_core!(JITModule)`, the batch backend `impl_cranelift_core!(ObjectModule)`. Where a task
below says "the generic signature is `<M: Module>(…)`", under Path B read it as "the macro expansion site for
this fn moves into the shared macro." The TDD steps, tests, and per-task commits are otherwise IDENTICAL.

> Path B keeps ONE source of truth (the macro body) — it is NOT a re-fork. The direction-check still requires
> that jit.rs no longer contain a hand-written copy of any covered fn.

---

## Task 1: Genericize `TranslationEnv` over `M: Module` (batch backend stays green)

**Path A. (Path B: this task instead introduces the `impl_cranelift_core!` macro skeleton and applies it to
`ObjectModule` only; jit.rs is untouched until Task 3.)**

**Files:**
- Modify: `compiler/ruxen_core/src/codegen/cranelift/translation_env.rs` (struct + impl head, lines 13–37)
- Modify: `compiler/ruxen_core/src/codegen/cranelift/mod.rs` (the `TranslationEnv { .. }` construction at
  line 365 needs no change — `M` infers — but the `use self::translation_env::TranslationEnv;` import and any
  turbofish must compile)
- Test: the existing `ruxen_core` codegen suite is the characterization backstop (no new test here — this is a
  type-level change with zero behaviour delta; the batch backend must keep compiling and passing).

- [ ] **Step 1: Run the batch codegen tests to capture the green baseline**

Run: `cargo test -p ruxen_core --test enum_codegen_debug --test codegen_unknown_method_rejected 2>&1 | tee tmp/test-cache/phase4-task1-baseline.log`
Expected: PASS. This is the characterization baseline — the genericization must not change a single result.

- [ ] **Step 2: Apply the generic struct + impl head**

Edit `translation_env.rs` exactly as in Task 0 Step 2 (the struct becomes `TranslationEnv<'a, M: Module>`,
`module: &'a mut M`; the three method bodies are UNCHANGED). Remove `use cranelift_object::ObjectModule;` if
nothing in this file still needs it.

> Sub-step for the implementer: `declare_func_in_func`, `declare_function`, `declare_data`, `define_data`,
> `isa()` are ALL `cranelift_module::Module` trait methods (verified against the cranelift_module API). No
> body changes — only the type of `self.module` generalises. If the compiler demands an extra bound (e.g.
> `M: Module` is insufficient for some method), that bound is the Task 0 NO-GO signal arriving late — STOP
> and switch to Path B.

- [ ] **Step 3: Re-run the batch codegen tests — verify identical green**

Run: `cargo test -p ruxen_core --test enum_codegen_debug --test codegen_unknown_method_rejected 2>&1 | tee tmp/test-cache/phase4-task1-after.log`
Expected: PASS, byte-identical result set to the baseline:
`diff <(grep "test result" tmp/test-cache/phase4-task1-baseline.log) <(grep "test result" tmp/test-cache/phase4-task1-after.log)` → empty.

- [ ] **Step 4: Commit**

```bash
git add compiler/ruxen_core/src/codegen/cranelift/translation_env.rs compiler/ruxen_core/src/codegen/cranelift/mod.rs
git commit -m "refactor(codegen): genericize TranslationEnv over M: Module

TranslationEnv<'a> -> TranslationEnv<'a, M: Module>; module field
&'a mut ObjectModule -> &'a mut M. Batch backend infers M = ObjectModule;
zero behaviour change (codegen tests identical). Prerequisite for sharing
the Cranelift core with the REPL JIT backend instead of forking it.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

## Task 2: Promote the module-AGNOSTIC helpers to `pub` (no `M` needed)

These 7 helpers take only `&mut FunctionBuilder` (or pure values) and never touch the module, so they are
shareable as-is by visibility alone. Promoting them first lets jit.rs delete its copies in Task 3 against a
stable, already-`pub` surface.

**Files:**
- Modify: `compiler/ruxen_core/src/codegen/cranelift/helpers.rs` — `ty_to_cranelift` (19), `cmpop_to_intcc`
  (69), `cmpop_to_floatcc` (80), `is_string_typed_value` (97), `is_string_mir_ty` (130), `simple_type_size`
  (148): `pub(super)` → `pub`.
- Modify: `compiler/ruxen_core/src/codegen/cranelift/emit.rs` — `emit_binop` (633), `coerce_value` (705),
  `coerce_value_signed` (719): `pub(super)` → `pub`.
- Modify: `compiler/ruxen_core/src/codegen/cranelift/mod.rs` — re-export the now-`pub` items so jit.rs can
  `use ruxen_core::codegen::cranelift::{ty_to_cranelift, emit_binop, coerce_value, …};`.

- [ ] **Step 1: Write a failing visibility/re-export pin test**

Create `compiler/ruxen_core/tests/cranelift_share_pin.rs` with a FIRST test that merely *names* the public
surface (compile-fails until the items are `pub` + re-exported):

```rust
// Pin: the shared Cranelift helpers must be reachable from outside the crate
// (the REPL JIT backend depends on them). If any goes back to pub(super),
// this fails to compile — re-fork prevention at the type level.
#[test]
fn shared_helpers_are_public() {
    use ruxen_core::codegen::cranelift::{
        cmpop_to_floatcc, cmpop_to_intcc, coerce_value, coerce_value_signed, emit_binop,
        is_string_typed_value, simple_type_size, ty_to_cranelift,
    };
    // Reference each to prevent unused-import elision; pure type-level assertion.
    let _ = (
        ty_to_cranelift as fn(&ruxen_core::hir::types::Ty) -> Option<cranelift_codegen::ir::types::Type>,
        cmpop_to_intcc as fn(ruxen_core::mir::nodes::CmpOp) -> _,
        cmpop_to_floatcc as fn(ruxen_core::mir::nodes::CmpOp) -> _,
        simple_type_size as fn(&ruxen_core::hir::types::Ty) -> usize,
    );
    let _ = (emit_binop, coerce_value, coerce_value_signed, is_string_typed_value);
}
```

> Implementer: align the exact fn-pointer coercions with the real signatures (read `helpers.rs`/`emit.rs`
> heads). The point is a compile-time "these are `pub` and re-exported" assertion, not runtime logic. Add
> `cranelift_codegen` as a `[dev-dependencies]` of `ruxen_core` ONLY if it isn't already a dep — check first
> (`grep cranelift_codegen compiler/ruxen_core/Cargo.toml`); it almost certainly is a normal dep.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --test cranelift_share_pin shared_helpers_are_public 2>&1 | tee tmp/test-cache/phase4-task2-red.log`
Expected: FAIL to compile — `function ... is private` / unresolved re-export.

- [ ] **Step 3: Promote visibility + re-export**

`pub(super)` → `pub` on the 9 helpers listed above. In `mod.rs`, extend the existing
`pub use runtime_sigs::runtime_signature;` neighbourhood with:

```rust
pub use emit::{coerce_value, coerce_value_signed, emit_binop};
pub use helpers::{
    cmpop_to_floatcc, cmpop_to_intcc, is_string_mir_ty, is_string_typed_value, simple_type_size,
    ty_to_cranelift,
};
```

> Do NOT promote `def_local`/`use_local`/`translate_instruction`/`translate_terminator`/`gen_value`/
> `coerce_call_args`/`build_signature` here — those carry `M` and are Task 4. Keep this task scoped to the
> zero-`M` set so its diff is trivially correct.

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p ruxen_core --test cranelift_share_pin shared_helpers_are_public 2>&1 | tee tmp/test-cache/phase4-task2-green.log`
Expected: PASS (1 test). Then confirm no batch regression:
`cargo test -p ruxen_core --test enum_codegen_debug 2>&1 | tee tmp/test-cache/phase4-task2-batch.log` → PASS.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/codegen/cranelift/helpers.rs compiler/ruxen_core/src/codegen/cranelift/emit.rs compiler/ruxen_core/src/codegen/cranelift/mod.rs compiler/ruxen_core/tests/cranelift_share_pin.rs
git commit -m "refactor(codegen): export module-agnostic Cranelift helpers as pub

ty_to_cranelift/cmpop_*/coerce_value*/emit_binop/is_string_*/simple_type_size
take only &mut FunctionBuilder and never touch the module, so they are
shareable by visibility alone. Re-exported from codegen::cranelift. Pin test
asserts the public surface so a regression to pub(super) fails to compile.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

## Task 3: Both-backends-identical pin test (re-fork tripwire) — RED first

**This test is the phase's anti-re-fork guarantee. It compiles a FIXED MIR fixture through BOTH the batch
`CodeGen` (ObjectModule) and the REPL `JITCodeGen` (JITModule) and asserts identical observable result. It
is written NOW (before jit.rs is gutted) so it pins current behaviour, goes green while jit.rs is still
forked, and stays green through the share — proving the share changed nothing observable.**

**Files:**
- Modify: `compiler/ruxen_core/tests/cranelift_share_pin.rs` (add the dual-compile test).

**Design constraint (load-bearing — the string-concat divergence):** the JIT fork special-cases
`BinOp::Add` on a string-typed operand to inline `ruxen_string_concat` (`jit.rs:964–980`), whereas the batch
`emit.rs` BinOp arm does NOT — it emits a plain `emit_binop` and relies on the MIR lowerer to have already
emitted a `Call ruxen_string_concat` for string `+`. **Therefore the pin fixture MUST NOT contain a string
`+`** (it would diverge by design until Task 4 reconciles it). Use an integer-arithmetic fixture so both
backends are expected to agree byte-for-byte from the start. The string-concat reconciliation is Task 4
Step 3b.

- [ ] **Step 1: Write the dual-compile pin test (expected GREEN immediately — it pins current parity)**

Add to `compiler/ruxen_core/tests/cranelift_share_pin.rs`:

```rust
// A fixed MIR fixture: `def add3(a: Int, b: Int) -> Int  =  (a + b) + 3`.
// Integer arithmetic ONLY — deliberately avoids string `+`, whose lowering
// differs between the batch backend (Call ruxen_string_concat from MIR) and
// the REPL fork (inlined at BinOp::Add). See plan Task 3 design note.
fn fixture_add3() -> ruxen_core::mir::nodes::MirFunction {
    // Implementer: build this MirFunction by hand from mir::nodes, OR reuse an
    // existing MIR-construction test helper in ruxen_core if one exists
    // (grep tests/ for `MirFunction {` builders). Two blocks max; one BinOp Add
    // of two params, one BinOp Add with an IntLiteral 3, one Return.
    todo!("hand-build add3 MIR — see mir::nodes::{MirFunction, MirBlock, MirInst, MirValue}")
}

#[test]
fn both_backends_agree_on_integer_fixture() {
    let mir = fixture_add3();

    // Batch path: compile through CodeGen(ObjectModule) and read back the
    // emitted CLIF for the function (or object-symbol disasm). The simplest
    // stable assertion is the Cranelift IR text of the defined function.
    let batch_clif = ruxen_core::codegen::cranelift::clif_for_test(&mir)
        .expect("batch CLIF");

    // JIT path: compile through JITCodeGen, run add3(7, 5), assert == 15.
    // (Execution equality is the strongest cross-backend assertion and does
    //  not require CLIF-text stability across cranelift versions.)
    let result = ruxen_repl::jit::run_int_fn_for_test(&mir, &[7, 5])
        .expect("jit run");
    assert_eq!(result, 15, "JIT add3(7,5)");

    // Cross-check: the batch backend's CLIF for the same fixture must contain
    // the same two iadd instructions (no string_concat, no divergent path).
    assert_eq!(batch_clif.matches("iadd").count(), 2, "batch emits 2 iadd, got:\n{batch_clif}");
}
```

> **Implementer obligation (explicit sub-step, not a placeholder):** add two tiny test-only entry points so
> the pin test can drive both backends without going through the full file pipeline:
> - `ruxen_core::codegen::cranelift::clif_for_test(&MirFunction) -> Result<String, String>`: a `#[cfg(test)]`
>   or `#[doc(hidden)] pub` shim that builds a `CodeGen`, runs `compile_function`, and returns
>   `ctx.func.display().to_string()` for the defined function. Gate it `#[cfg(any(test, feature = "test-hooks"))]`
>   or `#[doc(hidden)]` so it isn't public API surface.
> - `ruxen_repl::jit::run_int_fn_for_test(&MirFunction, &[i64]) -> Result<i64, String>`: builds a
>   `JITCodeGen`, compiles the fn, `get_finalized_function`, transmutes to the right `extern "C" fn` arity,
>   and calls it. (jit.rs already does the declare/define/finalize dance in `compile_repl_input`.)
> If the maintainer prefers NOT to add test hooks, fall back to: assert only the JIT execution result
> (`== 15`) and drop the batch-CLIF cross-check — the execution-equality half alone still trips on a re-fork
> that changes integer lowering.

- [ ] **Step 2: Run to verify it passes against the CURRENTLY-FORKED jit.rs**

Run: `cargo test -p ruxen_core --test cranelift_share_pin both_backends_agree 2>&1 | tee tmp/test-cache/phase4-task3-baseline.log`
Expected: PASS. This is the whole point — it is green BEFORE the share, so any drift introduced by gutting
jit.rs (Task 4/Task 5) turns it RED. Cache this as the immutable parity baseline.

> If it does not compile because `ruxen_repl` is a binary crate without a lib target, the implementer adds a
> minimal `lib.rs`/`pub mod jit;` exposure (or moves `run_int_fn_for_test` behind a `#[doc(hidden)]` pub in
> the existing lib target — check `src/ruxen_repl/Cargo.toml` for `[lib]`). Record which.

- [ ] **Step 3: Commit**

```bash
git add compiler/ruxen_core/tests/cranelift_share_pin.rs compiler/ruxen_core/src/codegen/cranelift/mod.rs src/ruxen_repl/src/jit.rs
git commit -m "test(codegen): pin both Cranelift backends to identical integer lowering

Compiles a fixed integer-arithmetic MIR fixture through the batch CodeGen
(ObjectModule) and the REPL JITCodeGen (JITModule), asserting identical
execution result + matching iadd count. Green against the still-forked jit.rs;
becomes the re-fork tripwire once the share lands. Fixture deliberately avoids
string '+', whose lowering legitimately differs pre-Task-4.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

## Task 4: Promote the `M`-carrying helpers to `pub` and rewrite jit.rs onto them

**The big deletion. jit.rs's 12 forked fns + `JITTranslationEnv` are replaced by imports of the shared core.
Path A: imports of generic `pub fn …<M: Module>` items, instantiated at `M = JITModule`. Path B: the
`impl_cranelift_core!(JITModule)` expansion replaces the bodies; jit.rs imports the macro.**

**Files:**
- Modify: `compiler/ruxen_core/src/codegen/cranelift/emit.rs` — promote `build_signature` (29), `def_local`
  (61), `use_local` (82), `translate_instruction` (97), `translate_terminator` (531), `gen_value` (601),
  `coerce_call_args` (769) to `pub`, with `<M: Module>` where they take `&mut TranslationEnv`/`&M`.
- Modify: `compiler/ruxen_core/src/codegen/cranelift/mod.rs` — re-export them.
- Modify (DELETE): `src/ruxen_repl/src/jit.rs` — remove `JITTranslationEnv` (749–917), `build_jit_signature`
  (919), `def_local` (601), `use_local` (617), `translate_instruction` (939), `translate_terminator` (1326),
  `gen_value` (1395), `coerce_call_args` (1426), `emit_binop` (1499), `cmpop_to_floatcc` (1488),
  `coerce_value` (1569), `coerce_value_signed` (1577), `ty_to_cranelift` (1606), `cmpop_to_intcc` (1650),
  `is_string_typed_value` (1661), `is_string_mir_ty` (1670), `simple_type_size` (1681), and the forked
  `runtime_signature` (~1762). Replace `compile_function_inner`'s body to construct the shared
  `TranslationEnv { module: &mut self.module, .. }` (M = JITModule) and call the shared
  `translate_instruction`/`translate_terminator`.

- [ ] **Step 1: Extend the pin test with the public-surface assertion for the `M` helpers (RED)**

Add to `cranelift_share_pin.rs::shared_helpers_are_public` (or a sibling test) names for the `M`-carrying
items so they must be `pub` + re-exported:

```rust
#[test]
fn shared_m_helpers_are_public() {
    use ruxen_core::codegen::cranelift::{
        build_signature, coerce_call_args, def_local, gen_value, translate_instruction,
        translate_terminator, use_local,
    };
    let _ = (build_signature::<cranelift_object::ObjectModule>); // instantiable at a concrete M
    let _ = (def_local, use_local, translate_instruction::<cranelift_object::ObjectModule>,
             translate_terminator::<cranelift_object::ObjectModule>, gen_value, coerce_call_args);
}
```

> Path B: this test instead asserts the macro is exported and expands at `ObjectModule`; adapt the names.

Run: `cargo test -p ruxen_core --test cranelift_share_pin shared_m_helpers_are_public 2>&1 | tee tmp/test-cache/phase4-task4-red.log`
Expected: FAIL to compile (private / not re-exported).

- [ ] **Step 2: Promote + thread `M` through the 5 module-touching helpers + `build_signature`/`def_local`/`use_local`**

In `emit.rs`:
- `build_signature<M: Module>(module: &M, func: &MirFunction) -> Signature` (was `&ObjectModule`).
- `translate_instruction<M: Module>(…, env: &mut TranslationEnv<M>)`, `translate_terminator<M: Module>(…, env: &mut TranslationEnv<M>)`.
- `gen_value`, `coerce_call_args`, `def_local`, `use_local`: these take `&mut FunctionBuilder` and (gen_value/
  coerce_call_args) values — check each: only those that take `env`/`module` need `<M>`. `def_local`/`use_local`/
  `gen_value` take NO env → no `<M>`, just `pub`. `coerce_call_args` takes `env.user_fn_param_tys` by reference
  in the batch call but the param is `&HashMap<String, Vec<Type>>` (not the env) → no `<M>`, just `pub`.

> Sub-step: confirm `gen_value` (601) and `coerce_call_args` (769) signatures — read them. They take
> `var_map`/`stack_slots`/`builder`/value args, NOT `env`. So they are zero-`M` and could even have been in
> Task 2; they are here only to keep the deletion atomic. Mark each `pub` (no `<M>`).

Re-export in `mod.rs`:
```rust
pub use emit::{
    build_signature, coerce_call_args, def_local, gen_value, translate_instruction,
    translate_terminator, use_local,
};
```

- [ ] **Step 3: Rewrite `jit.rs::compile_function_inner` to use the shared `TranslationEnv` + helpers**

Replace the `JITTranslationEnv { .. }` construction (jit.rs 479–486) with the shared one and swap the
local fn calls for the imported ones:

```rust
use ruxen_core::codegen::cranelift::{
    build_signature, coerce_value, def_local, translate_instruction, translate_terminator,
    ty_to_cranelift, is_string_mir_ty, TranslationEnv,   // TranslationEnv must be pub-exported too
};

// inside compile_function_inner, M infers to JITModule:
let mut env = TranslationEnv {
    module: &mut self.module,            // &mut JITModule
    declared_fns: &mut self.declared_fns,
    string_data: &mut self.string_data,
    string_counter: &mut self.string_counter,
    user_fn_param_tys: &self.user_fn_param_tys,
    vtable_data: &self.vtable_data,
};
```

> Sub-step: `TranslationEnv` is currently `pub(super)`. Promote it to `pub` and re-export from `mod.rs`
> (`pub use translation_env::TranslationEnv;`) — jit.rs constructs it directly, so it needs the fields `pub`
> too (they already are `pub(super)`; widen to `pub`). The field set is identical between
> `JITTranslationEnv` and the batch `TranslationEnv` (verified: module, declared_fns, string_data,
> string_counter, user_fn_param_tys, vtable_data) — the ONLY difference was the concrete module type, which
> `M` now absorbs.

- [ ] **Step 3b: Reconcile the string-concat divergence (delete the fork's special-case)**

The JIT fork's `BinOp::Add` string-concat inline (`jit.rs:964–980`) must NOT survive into the shared
`translate_instruction` — the batch backend relies on the MIR lowerer emitting `Call ruxen_string_concat`
for string `+`. Confirm (read `mir/lower`) that the lowerer DOES emit that Call for string `+` in the batch
path; if it does, the JIT fork's inline was compensating for the REPL feeding *un-lowered* string `+` into
codegen. Resolve by one of:
- (preferred) ensure REPL MIR is lowered the same way the batch backend is, so the shared `translate_instruction`
  never sees a string `+` BinOp — then the inline is dead and deleting it is correct;
- (fallback, only if REPL genuinely feeds string `+` as a raw BinOp) keep the string-concat handling but move
  it INTO the shared `emit.rs` BinOp arm guarded by `is_string_typed_value`, so BOTH backends gain it
  identically (this is a behaviour change for the batch backend — gate it behind a characterization test that
  proves the batch backend never reaches BinOp::Add with a string operand, i.e. the new arm is unreachable
  for batch and a no-op change for it).

Add a focused test capturing the chosen resolution (e.g. a REPL eval of `"a" + "b"` returns `"ab"`):

```rust
// in src/ruxen_repl/src/tests/ — string concat still works after the share.
#[test]
fn repl_string_concat_after_share() {
    // Use the existing REPL eval test harness (see src/ruxen_repl/src/tests/*).
    assert_repl_eval(r#""a" + "b""#, "ab");
}
```

> Implementer: this is the one place the share is NOT purely mechanical. Read `mir/lower` for string `+`
> handling and pick the resolution that keeps the batch backend byte-identical (the Task 3 pin test + the
> batch codegen suite are the backstop). Document which resolution in the commit body.

- [ ] **Step 4: DELETE the 12 forked fns + `JITTranslationEnv` + forked `runtime_signature` from jit.rs**

Remove every duplicated item listed in the Files block. jit.rs keeps ONLY: the `extern "C"` block, the
`JITCodeGen` struct, `new`, `compile_repl_input`, `compile_function`, `declare_function`, the mixin-vtable
emit methods, `compile_function_inner` (now using shared helpers), `register_runtime_symbols`,
`is_repl_symbol_allowed`, and the capture-shim wiring. Target: ~400 lines.

- [ ] **Step 5: Run the pin test + REPL suite + batch codegen suite — verify green**

Run:
```bash
cargo test -p ruxen_core --test cranelift_share_pin 2>&1 | tee tmp/test-cache/phase4-task4-pin.log
cargo test -p ruxen_repl 2>&1 | tee tmp/test-cache/phase4-task4-repl.log
cargo test -p ruxen_core --test enum_codegen_debug --test codegen_unknown_method_rejected 2>&1 | tee tmp/test-cache/phase4-task4-batch.log
```
Expected: ALL PASS. The pin test (`both_backends_agree_on_integer_fixture`) MUST still match its Task 3
baseline result exactly — that is the proof the share changed nothing observable:
`diff <(grep "test result" tmp/test-cache/phase4-task3-baseline.log) <(grep "both_backends_agree" tmp/test-cache/phase4-task4-pin.log)` (adjust grep).

- [ ] **Step 6: Confirm the deletion landed (net reduction, no leftover copies)**

```bash
git diff --stat HEAD~1..HEAD            # jit.rs must show a large deletion
grep -nE 'fn (translate_instruction|translate_terminator|gen_value|coerce_call_args|emit_binop|coerce_value|ty_to_cranelift|cmpop_to_intcc|cmpop_to_floatcc|is_string_typed_value)\b' src/ruxen_repl/src/jit.rs
```
Expected: the `grep` returns NOTHING (every forked fn is gone; jit.rs imports them instead).

- [ ] **Step 7: Commit**

```bash
git add compiler/ruxen_core/src/codegen/cranelift/emit.rs compiler/ruxen_core/src/codegen/cranelift/mod.rs compiler/ruxen_core/src/codegen/cranelift/translation_env.rs src/ruxen_repl/src/jit.rs src/ruxen_repl/src/tests/
git commit -m "refactor(repl): share the Cranelift core; delete the jit.rs fork

jit.rs no longer copies TranslationEnv + 12 emit/translate helpers from
codegen::cranelift; it constructs the now-generic TranslationEnv at
M = JITModule and calls the shared pub helpers. ~1,500 lines deleted. The
REPL-only string-concat-at-BinOp inline is reconciled (<resolution>); the
both-backends pin test stays green, proving no observable change.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff) — confirm jit.rs dropped ~1,500 lines and NO forked
      helper remains.

---

## Task 5: Resolve the `ruxen_noop_passthrough` global-rule ban (confirm load-bearing + document, or delete)

**Global rules ban `ruxen_noop_passthrough` as a test-bypass shim. The master risk register requires Phase 4
to investigate: confirm it is a real MIR/runtime sentinel and document why the ban does not apply, or delete
it.** Research already done during planning (record it; verify it holds):

- `ruxen_noop_passthrough` is a REAL `extern "C"` runtime symbol (`jit.rs:91`), referenced by the lowering
  via `runtime_name`/`lang_intrinsics.rs` for `yield`, `&str_as_str`, and the historical intrinsic fallback
  (`lang_intrinsics.rs:64,66,167,186`), and consumed by an explicit inline arm in BOTH Cranelift backends
  (`emit.rs:255`, `jit.rs:1094`) AND the LLVM backend (`llvm/emit/instructions.rs:167`). It is asserted by
  `runtime/tests_resolve.rs:63,66`. This is NOT a `#[ignore]`/mocked-HIR/dead-code test bypass — it is a
  load-bearing no-op intrinsic (the identity function used to lower `yield`-style passthroughs).

**Files:**
- Create: `docs/decisions/phase4-ruxen-noop-passthrough.md`
- Modify (only if the investigation finds a TRUE dead bypass): the offending site.

- [ ] **Step 1: Verify the sentinel is genuinely reachable (not a dead bypass)**

```bash
grep -rn "ruxen_noop_passthrough" compiler/ruxen_core/src src/ruxen_repl/src 2>&1 | tee tmp/test-cache/phase4-task5-refs.log
cargo test -p ruxen_core runtime::tests_resolve 2>&1 | tee tmp/test-cache/phase4-task5-resolve.log
```
Expected: refs in `lang_intrinsics.rs` (producer), both cranelift backends + llvm (consumer), and the
resolve tests (pin). The resolve test passing confirms `runtime_name("yield") == "ruxen_noop_passthrough"`
is live behaviour, not a bypass.

- [ ] **Step 2: Write the decision record**

Create `docs/decisions/phase4-ruxen-noop-passthrough.md` documenting: (a) the global-rule ban targets
*test-bypass shims* (mocked HIR, dead-code passthroughs that fake green tests); (b) `ruxen_noop_passthrough`
is instead a production no-op intrinsic emitted by `lang_intrinsics`/`runtime_name` for `yield` /
`&str_as_str` and lowered identically in all three backends; (c) it is pinned by `runtime/tests_resolve.rs`;
(d) therefore the ban does NOT apply and the symbol is retained. Cite the exact lines from Step 1's log.

> If Step 1 had instead shown the symbol is ONLY produced by a `_ => "ruxen_noop_passthrough"` catch-all that
> masks unimplemented methods (the `lang_intrinsics.rs:355,386` comments hint a historical fallback was
> already REMOVED), then the remaining uses are the legitimate `yield`/`&str_as_str` intrinsics — confirm the
> catch-all is gone (it is, per the comments) and that no NEW masking fallback exists. No code change needed;
> the decision record captures this.

- [ ] **Step 3: Commit**

```bash
git add docs/decisions/phase4-ruxen-noop-passthrough.md
git commit -m "docs(decisions): retain ruxen_noop_passthrough as a runtime intrinsic

The global-rule ban targets test-bypass shims (mocked HIR / dead-code
passthroughs). ruxen_noop_passthrough is a production no-op intrinsic emitted
by lang_intrinsics/runtime_name for yield and &str_as_str, lowered identically
in the cranelift + llvm backends and pinned by runtime::tests_resolve. Ban
does not apply; symbol retained. Decision recorded.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] Direction-check (/thermonuke on this task's diff)

---

## Task 6: Phase-4 final integration (phase gate)

**Files:** none (verification only).

- [ ] **Step 1: Run the full REPL + compiler suites once**

```bash
cargo test -p ruxen_repl 2>&1 | tee tmp/test-cache/phase4-final-repl.log
cargo test -p ruxen_core 2>&1 | tee tmp/test-cache/phase4-final-core.log
```
Expected: all green. Per global rule 41/42 this is the ONLY full-suite run in Phase 4; intermediate tasks
ran only their narrow tests.

- [ ] **Step 2: Confirm the both-backends pin test is green and unchanged from baseline**

Run: `cargo test -p ruxen_core --test cranelift_share_pin 2>&1 | tee tmp/test-cache/phase4-final-pin.log`
Expected: PASS, identical to `tmp/test-cache/phase4-task3-baseline.log`. The share is silent.

- [ ] **Step 3: Confirm the net reduction**

```bash
wc -l src/ruxen_repl/src/jit.rs                         # target ~400 (was 1,973)
git diff --stat <phase4-base>..HEAD                     # expect ~ -1,500 net
grep -cE '^\s*fn (translate_|gen_value|coerce_|emit_binop|cmpop_|ty_to_cranelift|is_string_)' src/ruxen_repl/src/jit.rs   # expect 0
```

- [ ] **Step 4: Full multi-agent `/thermonuke` sweep (authoritative phase gate)**

Invoke the `thermonuke` skill on the whole Phase 4 diff (`git diff <phase4-base>..HEAD`). It must confirm:
(a) the 12 forked helpers + `JITTranslationEnv` are GONE from jit.rs (deleted, not re-homed alongside the
shared ones); (b) jit.rs is now a thin JIT adapter (~400 lines: module construction + capture shims +
finalize); (c) the share introduced NO new `_ =>` catch-all in the shared core; (d) the both-backends pin
test exists and is green; (e) `ruxen_noop_passthrough` decision is recorded. Surface the report to the
maintainer.

- [ ] **Step 5: Report**

Report to maintainer: net line delta (`git diff --stat`), jit.rs final line count, the Task 0 GO/NO-GO
decision and which path was taken, the string-concat reconciliation chosen, full suites green (cite the
final-* logs), the both-backends pin test green and identical to baseline, and the statement: "The REPL JIT
backend now shares the batch Cranelift core verbatim; no observable behaviour changed (pin test + both full
suites green); the fork can no longer silently re-diverge (pin tripwire)." Await go-ahead for Phase 6.

---

## Self-Review (run before handing off)

**Spec coverage:** Genericize `TranslationEnv` (✓ Task 1), promote helpers (✓ Tasks 2 & 4), rewrite jit.rs as
adapter + delete fork (✓ Task 4), both-backends pin test (✓ Task 3), `ruxen_noop_passthrough` ban resolution
(✓ Task 5). The mandatory spike gates the phase (✓ Task 0) with explicit GO/NO-GO and a fully-specified Path
B macro fallback.

**Honesty about risk:** Task 0 is non-optional and reversible (throwaway branch). If the generic borrow-split
fights the `builder.func` borrow — the one real unknown the master risk register flags — Path B (macro-shared
core) is specified with identical TDD steps and the SAME re-fork tripwire, so the phase still lands ~1,500
deletion without a generic-lifetime gamble. The string-concat divergence (JIT inlines `ruxen_string_concat`
at `BinOp::Add`, batch does not) is called out as the single non-mechanical reconciliation (Task 4 Step 3b)
with two documented resolutions, both backstopped by the integer-only pin fixture and the batch codegen suite.

**Placeholder scan:** Two explicit implementer sub-steps carry `todo!`-shaped obligations: `fixture_add3()`
(hand-built MIR — not transcribed because the exact `mir::nodes` constructors weren't fully read during
planning; guessing them would be invented precision) and the two test-hook entry points (`clif_for_test` /
`run_int_fn_for_test`), with a specified fallback (execution-equality-only) if the maintainer rejects test
hooks. Every other code block is concrete against the read source (verified line refs: jit.rs fork comment 3–5,
`JITTranslationEnv` 749–756, string-concat 964–980, noop arm 1094; batch `TranslationEnv` 19–37,
`compile_function` 355–372, `emit.rs` BinOp 119–133 + noop 255, `helpers.rs` 19–140, `runtime_sigs.rs` 14–18).

**Type consistency:** `TranslationEnv<'a, M: Module>` field set matches the deleted `JITTranslationEnv`
field-for-field (module/declared_fns/string_data/string_counter/user_fn_param_tys/vtable_data) — the only
delta absorbed by `M`. The zero-`M` helpers (Task 2) and the `M`-carrying helpers (Task 4) are partitioned
correctly: `coerce_value`/`coerce_value_signed`/`emit_binop`/`cmpop_*`/`ty_to_cranelift`/
`is_string_typed_value`/`is_string_mir_ty`/`simple_type_size`/`def_local`/`use_local`/`gen_value`/
`coerce_call_args` take no env → no `<M>`; only `build_signature`/`translate_instruction`/
`translate_terminator` + the `TranslationEnv` methods need `M`. Path B preserves all signatures via macro
expansion at the two concrete module types.
