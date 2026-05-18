# 06.9 — dyn-Fn closure dispatch: make `any Fn(...)` actually callable

**Depends on:** #06.5 (sync I/O completeness); T9 of #06.5 surfaced
this gap.
**Reads:** `compiler/riven_core/src/mir/lower/expr/method_call.rs`
(closure-invocation lowering), `compiler/riven_core/src/codegen/runtime_table/mod.rs`
(call-name mangling fast-path), `compiler/riven_core/src/typeck/`
(coercion rules), `tests/release-e2e/cases/600_closure_handler_dispatch.rvn`
(the failing fixture from T9).

## Why this prompt exists

T9 of #06.5 surfaced a small but blocking compiler gap that
breaks every callback-registry pattern users would expect to be
able to write: Router (`app.get(path, handler)` dispatch), event
bus, observer pattern, plugin hooks, middleware stack, command
table.

What works today:
- Closure literals: `|x| x + 1` ✅
- Pass closure as `Fn(...)` arg: `def get(p: String, h: any Fn(...) -> ...)` ✅
- Store closures in array: `var hs: Array[any Fn(Int) -> Int] = Array.new; hs.push(|n| n + bias)` ✅

What's broken at codegen:
```riven
for h in hs.iter
  acc = acc + h.(5)     # codegen: no runtime symbol for `?T13_call`
end
```

The closure-invocation lowerer only emits an indirect call when
the receiver's static type starts with `Fn(` / `Fn[`. For
`any Fn(...)` the dyn-erased type appears as `?T13`, falls
through to a regular named call against `?T13_call`, and codegen
errors with "no runtime symbol for ?T::call."

Secondary gap (typeck):
```riven
let h: any Fn(Int) -> Int = |n| n + 1
# typeck: type mismatch: expected 'any Fn[Fn(Int) -> Int]', found 'Fn(Int) -> Int'
```

The closure → `any Fn(...)` unsize coercion exists in
argument-passing position but NOT in let-binding position.

This is a language gap, not a stdlib gap — fixing it doesn't
require new runtime fns or stdlib classes, only compiler work.

## Surface (the gap, exactly)

After this prompt:

```riven
# 1. let-binding coercion works:
let h: any Fn(Int) -> Int = |n| n + 1
h.(5)                                  # → 6

# 2. storage + iteration + dispatch works:
var hs: Array[any Fn(Int) -> Int] = Array.new
hs.push(|n| n + 10)
hs.push(|n| n * 3)
var acc = 0
for h in hs.iter
  acc = acc + h.(5)
end
puts "#{acc}"                          # → "30"  (15 + 15)

# 3. Router pattern works end-to-end:
class Router
  handlers: Array[any Fn(&Request) -> Response]

  def init -> ()
    @handlers = Array.new
  end

  def add(self, h: any Fn(&Request) -> Response) -> ()
    @handlers.push(h)
  end

  def dispatch(self, req: &Request) -> Response
    @handlers[0].(req)                 # or iterate per route, etc.
  end
end
```

## Touch list (compiler-only; no stdlib, no runtime C)

### 1. `mir/lower/expr/method_call.rs` (~lines 844–893)

The `is_fn_call` recognizer currently checks the receiver's
`type_name` against the prefixes `Fn(` / `Fn[` / `&Fn(` /
`&Fn[`. Extend it to also recognize the dyn-erased forms:
`any Fn(` / `any Fn[` / `&any Fn(` / `&any Fn[` — and the
mangled spelling the resolver produces for these (`?T*` for
opaque dyn-mixin existentials).

In the matched branch, lowering emits an INDIRECT call rather
than a named call — same shape as the existing `Fn(...)`
indirect path. The receiver is the closure pointer; the call
target is the closure's invoke-method pointer. For dyn-erased
receivers the v-table is already present (the unsize coercion
in §3 below installs it); the indirect call dereferences it.

### 2. `codegen/runtime_table/mod.rs` (~line 307)

The `runtime_name` fast-path currently maps `Fn(...)_call` /
`Fn[...]_call` to `riven_noop_passthrough` so the lowering's
named-call fallback survives the dispatch table. Add a
parallel arm:

- Any `?T*_call` mangling (the placeholder for an erased dyn-Fn
  receiver) → `riven_noop_passthrough` too.

This is a belt-and-suspenders guard. The §1 fix above should
mean lowering NEVER emits a `?T*_call` named call in the first
place — but if a missed path slips through, this prevents a
hard codegen error and produces a deterministic runtime panic
instead (which can then be debugged).

### 3. `typeck/` — closure → `any Fn(...)` coercion in let-RHS

The coercion already exists in argument-passing position. Find
the coercion site (grep for `any Fn` or `unsize` in
`typeck/coerce*.rs` / `typeck/check_expr*.rs`) and confirm it's
guarded by "arg-position only." Lift the guard so the same
coercion runs in:

- `let x: any Fn(...) -> ... = closure_literal_or_concrete_fn_value`
- `var x: any Fn(...) -> ... = ...` (same as let)
- `@field_assign = ...` (struct/class field initializer in `init`)
- `array_push(any_fn_array, concrete_closure)` — this already
  works in arg-position; double-check after the fix.
- `return closure_literal` from a fn whose return type is `any Fn(...)`.

The coercion installs the v-table that §1's indirect call later
dereferences. If §1 is correct but §3 is incomplete, the
let-binding case still fails at typeck; if §3 is correct but §1
is incomplete, the let binds but the call still fails at codegen.

Both must land in the same commit so the gates align.

## TDD

The failing fixture from T9 (`600_closure_handler_dispatch.rvn`)
is the regression gate. Two more pin tests in a new file:

`compiler/riven_core/tests/closures_dyn_dispatch.rs`:

- `dyn_fn_let_binding_coerces` — `let h: any Fn(Int) -> Int = |n| n + 1`
  binds without typeck error and calling `h.(5)` returns 6.
- `dyn_fn_array_store_and_dispatch` — the failing 600_ fixture
  from T9, captured here as a pin test instead of an e2e.
- `dyn_fn_router_pattern` — class with `Array[any Fn(...)]` field,
  `add` + `dispatch` methods. Asserts the surface in §"Surface"
  case 3 above produces "30" as documented.
- `dyn_fn_return_from_function` — `def make_adder(n: Int) -> any Fn(Int) -> Int = |x| x + n; make_adder(3).(4)` returns 7.

The e2e fixture `tests/release-e2e/cases/600_closure_handler_dispatch.rvn`
that T9 wrote (but couldn't ship because the rung failed) should
ship as part of this prompt's deliverables, with a matching
`.out` expected output file.

## Reserved error codes

This prompt doesn't introduce new error codes. The existing
"type mismatch" diagnostic that T9 saw becomes unreachable for
the let-binding case once §3 lands. If you find yourself
wanting an error code (e.g. "this dyn-Fn coercion site can't
be lifted yet" for some edge case), prefix-reserve **E0730–E0734**
for future dyn-Fn limit diagnostics.

## Definition of done

- [ ] `let h: any Fn(...) -> ... = closure` typechecks (no
      `type mismatch`).
- [ ] `Array[any Fn(...)]` storage + iteration + `.(args)`
      dispatch codegens and runs.
- [ ] `tests/release-e2e/cases/600_closure_handler_dispatch.rvn`
      compiles and runs, producing the documented stdout.
- [ ] `closures_dyn_dispatch.rs` pin tests all green.
- [ ] No regression in pre-existing closure tests
      (`28_closures.rvn`, `88_closure_do_end.rvn`,
      `89_closure_capture_immut.rvn`, `90_closure_capture_var.rvn`,
      `91_move_closure.rvn`, `92_closure_as_arg.rvn`,
      `93_yield_block.rvn`).
- [ ] CHANGELOG bullet under `## [Unreleased] ### Fixed`:
      "compiler: `any Fn(...)` closures are now dispatchable via
      indirect call; works in let-binding, array storage, class
      field, and return-position contexts. Unblocks Router /
      event-bus / observer-pattern code."
- [ ] If applicable, `docs/specs/syntax/ruby-naming.spec.md` is
      updated to remove the "argument-position only" caveat from
      the `any Fn(...)` coercion docs.

## Anti-goals

- **Adding new runtime fns.** This is a compiler-side fix. The
  closure invoke-method pointer is already present in the
  closure's flat-heap layout from earlier closure work; no new
  C runtime symbols required.
- **Changing the `Fn` / `FnVar` / `FnOnce` mixin definitions.**
  Those are correct; the gap is purely in how `any Fn(...)`
  dyn-erased values get LOWERED and CALLED.
- **Trait-object boxing semantics.** `any Fn(...)` is already
  the dyn-trait spelling; the v-table already exists for the
  arg-position case. Don't re-architect — just lift the existing
  machinery to the new positions.
- **Lifetime / borrow-checker work on captured-by-ref closures.**
  Existing closure capture rules stay. If the dyn coercion in
  §3 reveals a latent capture / borrow issue, file it as a
  follow-up — don't expand scope into borrow-checker
  territory.
- **`FnOnce` / `FnVar` dyn dispatch.** Out of scope. v1 only
  needs `any Fn(...)` (call-multiple-times, immut captures) to
  unblock the Router pattern. Dyn `FnVar` / `FnOnce` can land
  in a follow-up if user pull warrants it.

## Estimated scope

- §1 (lowering): ~2 hours including pin test
- §2 (runtime_table fast-path): ~30 minutes
- §3 (typeck coercion lifting): ~3 hours including 3-4 pin
  tests across let / field / return positions
- E2E fixture revival + workspace verify: ~30 minutes

Total: **~1 day for one principal-rust-engineer agent.**

## Why this comes before #10 (LSP)

LSP itself doesn't need `any Fn(...)` dispatch. But every realistic
Riven program built with the LSP — most of them — does: any code
that stores callbacks, registers handlers, or implements an event
loop reaches for this surface immediately. Shipping LSP on a
language where Router is unimplementable is a bad first
impression. Land #06.9 first; LSP follows naturally.
