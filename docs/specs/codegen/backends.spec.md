# Spec — Codegen backends (Cranelift + LLVM)

**Source docs:**
[docs/requirements/](../../requirements/) (no single doc — see ROADMAP §codegen).

**Status:** Cranelift backend default since Phase 1; LLVM 18 backend
behind `--features llvm` since Phase 4.

Riven compiles MIR through one of two backends.  Both must accept the
same MIR and produce binaries that pass the same E2E suite.

---

## B1 — Default backend is Cranelift

A bare `rivenc input.rvn` produces a binary via Cranelift.  Optimised
build time is dominated by Cranelift; debug build time is dominated
by C-runtime compilation.

## B2 — `--features llvm` switches to LLVM 18

When the workspace is built with `--features llvm`, `rivenc --llvm`
produces a binary via LLVM 18.  The CLI flag exists; the backend is
feature-gated so default builds don't need llvm-sys.

## B3 — Both backends produce byte-identical stdout on the E2E fixture set

For every `tests/release-e2e/cases/<fixture>.rvn`, the Cranelift and
LLVM backends must produce a binary whose stdout matches
`tests/release-e2e/expected/<fixture>.out` exactly.

(Today the pin asserts the LLVM IR verifies for every fixture; the
stdout-equality run lives in CI release-e2e and is gated to the LLVM
job.)

## B4 — Optimisation levels `-O0` / `-O1` / `-O2` produce correct output

For LLVM, every optimisation level (`OptLevel::None`, `Less`,
`Default`) must keep stdout byte-identical to the reference.
Asserts the codegen is robust under inlining, DCE, and other
LLVM passes.

## B5 — Unknown method calls are rejected at codegen

**Given** source that calls an inferred method name no backend has
wired (`some_value.flat_map(...)` today)
**Then** codegen rejects with a clear diagnostic; the rejection
happens before any binary is emitted.

## B6 — String literals lower through `riven_string_from` wrapper

String literals are not emitted as bare `char*` pointers — they're
copied into a fresh heap allocation via `riven_string_from` so that
ownership semantics work uniformly (the same value can be moved /
dropped / re-bound).

## B7 — Unreserved identifiers do not collide with runtime symbols

Riven user code may use names like `len`, `push`, `clone`, etc., as
local bindings or method names without colliding with the runtime
helpers (`riven_string_len`, `riven_vec_push`, `riven_string_clone`,
…).  E2E fixture `135_unreserved_idents.rvn` exercises a long list.

---

## Pin tests

| Behaviour | Test fn                                                | File                              |
|-----------|--------------------------------------------------------|-----------------------------------|
| B1        | every test that calls `codegen::compile(...)`          | (workspace-wide)                  |
| B2        | `assert_backends_identical` (gated to LLVM job)        | `llvm_backend.rs`                 |
| B3        | `llvm_ir_verifies_all_fixtures` (LLVM IR verifier)     | `llvm_backend.rs`                 |
| B3 + B4   | `all_opt_levels_correct`                               | `llvm_backend.rs`                 |
| B5        | `compile_fails_when_calling_unimplemented_iter_flat_map` + `runtime_name_rejects_unknown_inferred_method` | `codegen_unknown_method_rejected.rs` |
| B6        | `string_literal_lowers_through_string_from_wrapper`    | `string_literal_wrap.rs`          |
| B7        | `e2e_135_unreserved_idents`                            | `unreserved_idents_runtime.rs`    |

The cross-backend equality assertion (`assert_backends_identical`) is
the load-bearing pin: any optimisation or codegen change that makes
the two backends diverge fails this.

---

## Out of scope (v2)

- A third backend (e.g. WASM, GCC).  Tier-4 / future.
- Profile-guided optimisation.
- Cross-compilation flag surface (`--target`).
- Incremental codegen / sccache integration.
