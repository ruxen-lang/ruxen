# Decision: retain `ruxen_noop_passthrough` as a runtime intrinsic

**Status:** Accepted (Phase 4, Task 5).
**Question:** The global TDD-discipline rules ban `ruxen_noop_passthrough` as a
test-bypass shim. Does that ban apply to the `ruxen_noop_passthrough` symbol in
this codebase?

**Answer: No. The ban does not apply. The symbol is retained.**

## What the ban targets

The ban targets *test-bypass shims*: mocked HIR, `#[ignore]`'d tests,
`riven_noop_passthrough`-style dead-code passthroughs, and `_ => <identity>`
catch-alls that make a test go green by silently accepting unimplemented
behaviour instead of failing. The harm is a silent-failure path that masks
missing functionality.

## What `ruxen_noop_passthrough` actually is

It is a **production no-op (identity) runtime intrinsic** — `fn(i64) -> i64`
returning its argument — emitted by the codegen name-resolution layer for a
small set of *intentional* passthrough lowerings, and lowered inline (no C
call) by all backends. It is not a test artifact and is not reachable from any
masking catch-all.

### Producer (real intrinsic mappings)
`compiler/ruxen_core/src/codegen/lang_intrinsics.rs` (`runtime_name`):
- L64 `"yield" => Ok("ruxen_noop_passthrough")` — the per-call placeholder for
  invoking a block argument; backends inline the closure call elsewhere, so the
  `yield` callee itself is a passthrough.
- L66 `"&str_as_str" => Ok("ruxen_noop_passthrough")` — a *true semantic
  identity* (`&str` → `&str`), explicitly commented as "not a stub".
- L167 / L186 — closure-invocation dispatch (`Fn(...)_call`, `any Fn(...)`,
  `?T*_call`): the MIR lowerer normally emits a direct indirect call; these arms
  are belt-and-suspenders so a missed lowering produces a deterministic
  passthrough rather than a hard codegen error.

### The masking catch-all was already REMOVED (not present)
`lang_intrinsics.rs` L355-358 and L385-391 document the P0.5 change: the
historical `_ => "ruxen_noop_passthrough"` fallback that accepted *any* method
on an inferred / `?T` / `Result` type and silently returned the receiver was
removed. The unknown-method arms now return
`Err(unresolved_method_error(...))` (verified: `_ => Err(...)`, never
`Ok("ruxen_noop_passthrough")`). `runtime/mod.rs` L50 carries the same note.
So there is no surviving catch-all that masks unimplemented methods — exactly
the construct the ban exists to prevent.

### Consumer (real inline lowering, single source after Phase 4)
- `compiler/ruxen_core/src/codegen/cranelift/emit.rs` L254 — lowers a
  `Call ruxen_noop_passthrough` to "materialise the first argument into dest"
  (the identity), with **no C call emitted**. After Phase 4's Cranelift-core
  share, this is the SOLE cranelift consumer; the previously-forked duplicate in
  `src/ruxen_repl/src/jit.rs` was deleted with the rest of the fork.
- `compiler/ruxen_core/src/codegen/llvm/emit/instructions.rs` L167 — the
  identical inline lowering on the LLVM backend.
- `src/ruxen_repl/src/jit.rs` L97 (`extern "C"` decl) + L714
  (`register_runtime_symbols`) — registers the C runtime symbol so the REPL JIT
  can resolve it via `dlsym`.

### Pinned by tests (live behaviour, not a bypass)
`compiler/ruxen_core/src/codegen/runtime/tests_resolve.rs` L63/L66 assert
`runtime_name("yield").unwrap() == "ruxen_noop_passthrough"` and the
`&str_as_str` identity. These tests pass (Task 5 run:
`tmp/test-cache/phase4-task5-resolve.log`, 6 passed). The same file's L10 pins
that an *unknown* method now errors instead of resolving to the passthrough —
i.e. the tests actively guard against the banned masking behaviour returning.

## Conclusion

`ruxen_noop_passthrough` is a load-bearing identity intrinsic for `yield` /
`&str_as_str` / closure-dispatch passthroughs, lowered inline and identically in
the Cranelift and LLVM backends, pinned by `runtime::tests_resolve`. The
silent-failure catch-all that the ban targets was already removed and is
test-guarded against reintroduction. The ban therefore does not apply; the
symbol is retained unchanged. No code change is required by this decision.
