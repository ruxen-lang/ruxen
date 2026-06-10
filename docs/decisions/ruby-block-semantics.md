# ADR: Ruby-block semantics for Ruxen (`&block` / `yield` / `block_defined?`)

Status: Accepted (2026-06-10)
Branch: `feat/drop-elaboration`
Supersedes: the "DESIGN NOTE — closure/block model rework" draft in
`docs/dev/gui-stack-v1-issues.md` (Q2 section).

## Context

Ruxen already had **two** partially-overlapping block mechanisms before this work:

1. **Implicit `yield` blocks.** `resolve/yield_scan.rs` pre-scans every function
   body for `yield`; if found, `resolve/funcs.rs` synthesizes a trailing
   `__block: Fn(args…) -> Unit` parameter, `yield x` desugars to `__block.(x)`,
   and a caller's trailing `do…end` / `{ }` forwards as the last argument. This
   powers the Tester DSL and the quiver builder DSL (`widget do |w| … end`).
   **The block's return type was hard-forced to `Unit`** — v1 deliberately never
   consumed a block's value.

2. **Explicit closure-param free fns / methods.** `def runit(f: any Fn[Fn() -> nil])`
   plus `runit do … end` forwards the block as an ordinary closure argument. The
   old "do…end to a free fn segfaults" landmine (ledger Q3) was already FIXED
   (pin `tests/release-e2e/cases/642_free_fn_do_block`) before this work — verified
   green against a fresh build at the start of this pass.

What was **missing** versus the user's spec:

- No spec-shaped **`&block: Fn[(T…) -> R]` declaration** (an unused, untested
  vestigial `&block: Block(...)` path existed with a non-spec type spelling).
- No **`block_defined?`** presence predicate.
- No **optionality + runtime guard** when `yield` runs with no block.
- `yield`'s value was always `Unit`, so `let r = yield(4)` was impossible.

### Representation facts (verified in-tree)

- A closure literal / block lowers to an **8-byte pointer to a heap 16-byte pair
  `{fn_ptr@0, captures_ptr@8}`** (`mir/lower/expr/closure.rs:13-15, 355-381`).
  Call ABI: load both fields, then
  `CallIndirect(fn_ptr, [captures_ptr, …args])` — `captures_ptr` is arg[0]
  (`mir/lower/expr/method_call.rs:1384-1412`). Drop ABI: `drops.rs:148-157`.
- An **`any Fn`** (mixin-typed, dynamically dispatched) value is a *different*
  16-byte fat value (data_ptr + vtable_ptr). **Q2's garbage is that 16-byte fat
  value crushed into a 1-word enum payload slot** — half is lost. The
  closure-pair-pointer (8 bytes) does NOT have this problem.

## Decision

### D1 — Representation: closure-pair-pointer + null-fn-ptr presence sentinel

A `&block` parameter is the **existing 8-byte closure-pair-pointer**, NOT an
`any Fn` value. Presence is encoded as a **null sentinel**:

- block present  → a non-null pointer to `{fn_ptr, captures_ptr}`.
- block absent   → the null pointer (`0`).

`block_defined?` lowers to `block_ptr != 0`. `yield` / `block.(args)` guards the
pointer: a null pointer at a `yield` site triggers a runtime panic with a
LocalJumpError-style message naming the enclosing function (see D5).

This needs **no new MIR node** and **never touches the broken `any Fn`
enum-payload path**, so the block slot is *independent of Q2*. Q2 stays open and
is NOT fixed by this work (it is the separate `any Fn`-in-enum-payload-slot
layout bug); this ADR records that scoping explicitly. The clean block path does
not reuse that path, satisfying the spec's "must NOT reuse that broken path"
requirement.

### D2 — `yield`'s value is the block's return type `R` (for explicit `&block`)

When a function declares `&block: Fn[(T…) -> R]`, `yield` is an **expression of
type `R`**, so `let r = yield(4)` type-checks against `R`. For the **bare-`yield`
path** (a function that `yield`s but has no explicit `&block` declaration), the
synthetic block keeps the historical **`Unit` return default** — this preserves
every existing builder/each DSL that yields mid-body and discards the value, and
avoids `could not infer type for __block`. Rule: *explicit `&block` decl ⇒ typed
`R`; bare `yield` ⇒ `Unit`.*

### D3 — Single trailing-block attachment rule for `do…end` AND `{ }`

Ruxen keeps its **one** existing attachment rule: a trailing `do…end` or `{ }`
block attaches as the implicit last argument, identically for free functions and
methods, with no Ruby do-vs-brace precedence split. `{ }` after a call is the
block; `do…end` after a call is the block. (Ruby's lower-precedence `do…end`
binding is intentionally NOT replicated — Ruxen has a single rule, pinned.)

### D4 — `&block` must be the last parameter (E1119)

`&block` is enforced to be the final parameter. A `&block` followed by any
positional parameter is a hard error **E1119** ("block parameter must be the last
parameter"), registered in `diagnostics::codes::REGISTRY` with `docs/errors/E1119.md`.

### D5 — Optionality + runtime guard

Every `&block` parameter is **optional** at call sites (Ruby semantics). Calling
without a block is legal: `block_defined?` is `false`; a `yield` reached with no
block **panics at runtime** with a message of the form
`yield called without a block in `funcname`` and a non-zero exit. We do NOT add a
compile-time "every call site is blockless" proof in Tier 1 (kept simple, runtime
check only).

### D6 — `block_defined?` (+ `block_given?` alias)

`block_defined?` parses as a no-receiver builtin predicate, types as `Bool`, and
lowers to the null-pointer test on the enclosing function's block slot.
`block_given?` is accepted as a trivial alias (same lowering).

### D7 — Nested `yield` rule

`yield` refers to the **lexically-enclosing function's** block. A `yield` written
inside a *closure* body cannot be soundly attributed to that closure in Tier 1
(the closure is a separate frame with its own ABI), so it is a **clean compile
error**, not a miscompile. (`yield_scan` already declines to attribute a `yield`
inside a nested closure to the enclosing function — `Closure(_) => None`.)

### D8 — Type spelling: `Fn[(T…) -> R]` canonical, `Fn(…) -> R` back-compat

The **canonical** block-type spelling is `Fn[(T…) -> R]` (square brackets, matching
Ruxen's generic convention `Array[Int]`, `State[T]`). The ADR, E1119 message,
tutorial, and all new pins use it. The existing `Fn(…) -> R` / `any Fn[Fn(…) -> R]`
spellings stay accepted (existing fixtures must not break). `ruxen fmt` PRESERVES
whichever **type spelling** was written — `Fn[...]` stays `Fn[...]`, `Fn(...)`
stays `Fn(...)` — **no normalization rewrite** of the type form (avoids a fourth
fmt-destructiveness incident after Q23/Q30/Q34). This is carried by a
semantically-inert `bracketed: bool` on `TypeExpr::Function`.

Scope note on the CALL-SITE block form (`do…end` vs `{ }`): the formatter's
choice there is **pre-existing and content-driven** — a multi-statement block
formats as `do…end`, a single-statement block that fits formats as `{ }` —
keyed on the block's content, not the authored token (a `ClosureExpr` does not
record which delimiter was written). This feature does **not** change that
policy; doing so would risk the corpus idempotency the prior incidents were
about. The invariant we hold is `format(format(x)) == format(x)` on the new
block surface (pinned), not byte-identity with the authored delimiter.

## Out of scope

**Staged (Tier 2):** `&` block-forwarding (`g(&block)` / anonymous `&`), `next`
as block-value, `&:symbol` to-proc sugar, numbered params / `it`.

**Rejected (with rationale):**
- Non-local `return` / Ruby's `break`-exits-the-yielding-method — fights
  ownership + drop elaboration (a block's `return` would have to unwind the
  yielding frame's drops out of order).
- `redo`.
- Lenient arity — Ruxen is typed; exact arity with a clear diagnostic.
- `instance_eval` self-rebinding.

## Consequences

- Q3 landmine ("do…end to free fn segfaults") was already fixed; the quiver
  `CLAUDE.md` landmine entry warning against it is now stale and should be deleted
  by the migration wave (noted in the ledger).
- Q2 remains open and independent; the block slot does not depend on its fix.
- The Tester framework (`do…end` on methods everywhere) is the strongest existing
  regression net and must stay green unchanged.

### Known Tier-1 limitation — RESOLVED (2026-06-10)
A **paren-less, blockless** call to an optional-block **method** (`w.frame`, no
parens and no block) previously did not fill the block slot: it parses as a
`FieldAccess` whose no-arg method path did not append the `nil` block default,
so MIR emitted one too few arguments and **crashed the arity verifier**
(`__closure_*: got 1, expected 2`). `w.frame()` (parens) and any block-bearing
form worked, and free functions had no such gap (a blockless `render` works).

**Fix (the "fill defaults at MIR" option):** the no-arg method route in
`mir/lower/expr/field_access.rs` now appends the resolved method's trailing
default arguments — the MIR mirror of typeck's `append_method_default_args`,
which the parens `MethodCall` path already runs. The new helper
`Lowerer::method_trailing_default_sentinels` (`mir/lower/mod.rs`) looks up the
resolved method's signature and materializes a null closure-pair-pointer
sentinel (`Literal::Int(0)`, the same value `NullLiteral` lowers to for a
non-`Option` type) for each defaulted trailing param the call did not supply.
So `w.frame` and `w.frame()` now lower **identically**, consistent with the
earlier paren-less auto-call fix for regular defaults
(`autocall_uses_real_default_not_null`). We chose the MIR default-fill over the
"rewrite FieldAccess into MethodCall" option to keep the change off the broad
paren-less-call corpus. Pins: release-e2e `921_block_optional_method_parenless`
(RUN + assert stdout; a revert CRASHES at MIR, not merely a stdout mismatch),
`compiler/ruxen_core/tests/ruby_block_semantics.rs`
(`parenless_blockless_method_call_fills_block_slot` +
`explicit_block_param_on_method` extended with `w.build`).
