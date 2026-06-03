# Phase 4 Task 0 — `TranslationEnv<M: Module>` de-risking spike result

**Decision: GO — Path A (genericize). Tasks 1–5 use the generic path.**

## What was proven

The spike genericized `TranslationEnv<'a>` to `TranslationEnv<'a, M: Module>`
(`module: &'a mut M`, dropping the concrete `&'a mut ObjectModule`), threaded
`<M: Module>` through `build_signature`, `translate_instruction`, and
`translate_terminator` in `emit.rs`, and dropped the
`use cranelift_object::ObjectModule;` import from both `translation_env.rs` and
`emit.rs`. The three env method bodies (`create_string_data`,
`get_or_declare_func`, `declare_runtime_func`) were left UNCHANGED.

`cargo build -p ruxen_core` then succeeded with **0 errors, 0 warnings**
(`tmp/test-cache/phase4-task0-spike-build.log`).

## Why this is a GO (the borrow-split holds generically)

The crux risk was the line, reachable from `get_or_declare_func`:

```rust
let func_ref = self.module.declare_func_in_func(func_id, builder.func);
```

Here `self.module: &'a mut M` and `builder: &mut FunctionBuilder<'_>` (which
owns `&mut ctx.func`) are both live. The open question was whether the *generic*
`&'a mut M` form would tie itself to the `builder.func` borrow and fail the
borrow checker, or force a bound beyond `Module`.

It did not. `M: Module` alone is sufficient. `declare_func_in_func`,
`declare_function`, `declare_data`, `define_data`, and `isa()` are all
`cranelift_module::Module` trait methods, so the bodies type-check verbatim
against the generic receiver. The `&'a mut M` (module) and `&mut FunctionBuilder`
(ctx.func) remain provably-disjoint `&mut` paths into the caller's `self`,
exactly as they were for the concrete `ObjectModule` — genericizing `M` did not
introduce any new aliasing or lifetime obligation.

Confirmed:
- No `ObjectModule` concrete name remains required in the shared core
  (`grep ObjectModule translation_env.rs emit.rs` → empty after the spike edits).
- The batch backend's `compile_function` inferred `M = ObjectModule` at the
  `TranslationEnv { module: &mut self.module, .. }` construction site and the
  `build_signature(&self.module, ..)` / `translate_instruction(..)` calls with
  **zero call-site changes**.

## Disposition

Per the plan, the spike was a throwaway: its edits to `translation_env.rs` and
`emit.rs` were reverted (the working tree is back to the concrete `'a`-only
form). Only this decision record is committed. Tasks 1–5 re-apply the
genericization under strict TDD (Path A), starting from Task 1.

Path B (the `macro_rules! impl_cranelift_core` fallback) is **not** taken.
