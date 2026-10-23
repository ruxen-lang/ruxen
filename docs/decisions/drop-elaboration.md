# Decision: Drop elaboration via MIR-emitted free calls; `MirInst::Drop` stays an inert marker

**Status:** Accepted (documents already-landed work). Supersedes the stale
P0.2 framing in `docs/requirements/ROADMAP.md` and
`docs/requirements/tier1_04_drop_copy_clone.md` §2.3.

**Question:** How is `MirInst::Drop` lowered so heap-owning locals are freed
deterministically at scope exit, exactly once, without dropping a value that was
moved out (no double-free / use-after-free)? And which backend owns the work?

## TL;DR — this is already done and proven

The ROADMAP claims (P0.2) "`MirInst::Drop` is emitted but both codegen backends
silently discard it, so every program leaks heap memory until exit," citing
`codegen/cranelift.rs:692-698` and `codegen/llvm/emit.rs:790-792`. **That
description is stale.** Those file paths no longer exist (the backends were split
into `codegen/cranelift/` and `codegen/llvm/emit/`), and the behaviour they
describe was fixed and committed before this branch
(`feat/drop-elaboration`, HEAD `35d80b7`). The working tree equals HEAD for all
tracked files; no further code change is required to close the single-heap-local
normal-scope-exit case the prompt scoped — it is already correct, committed, and
covered by a leak-audit pin asserting `outstanding == 0`.

This ADR records the architecture so the next reader does not re-implement a
working pass from the stale premise and risk introducing a double-free (a wrong
Drop is worse than none).

## Chosen lowering approach

`MirInst::Drop { local }` is an **inert marker** in both backends, by design:

- Cranelift: `codegen/cranelift/emit.rs:452`
- LLVM: `codegen/llvm/emit/instructions.rs:472`

The *real* destructor and free work is emitted as **ordinary `MirInst::Call`
instructions during MIR lowering**, by `insert_drops` in
`compiler/ruxen_core/src/mir/lower/drops.rs`. Because the frees are plain MIR
calls (not backend-special-cased), **both backends honour Drop identically and
for free** — there is no "LLVM still discards Drop" gap to stage. Keeping
`MirInst::Drop` as a no-op marker (rather than deleting it) preserves the MIR
shape other passes/snapshots expect and documents the drop point.

For each block ending in `Terminator::Return`, `insert_drops` appends, in
**reverse declaration order** (LIFO), for each drop-eligible local:

1. the user destructor `{Class}_drop(self)` if the type includes `Drop`
   (resolved through the FFI-alias map so `def drop as "ruxen_…"` reaches the
   right C symbol), then
2. a type-directed free call:
   - `Ty::String` → `ruxen_string_free`
   - `Ty::Array(elem)` → `ruxen_vec_drop(v, desc(elem))`
   - `Ty::Map(k,v)` → `ruxen_hash_drop(h, desc(k), desc(v))`
   - `Ty::Set(elem)` → `ruxen_set_drop(s, desc(elem))`
   - `Ty::Class/Struct/Enum` → `ruxen_dealloc`

   `desc` is an element descriptor (`drops::elem_desc`, the `RUXEN_ELEM_*`
   block in `runtime.h`): a pre-order stream of 4-bit tags that the C side
   follows recursively through `ruxen_elem_drop`, so nested containers are
   freed to any depth. A `Hash` key or `Set` element is described as a
   string or a plain word only, matching how those tables copy keys. The
   eligibility filter is `heap_free_callee(ty).is_some()`, so no other type
   reaches this point.

Closures use the same MIR-call architecture. Lowering attaches a synthesized
`__closure_N_drop(pair)` function to the fresh pair local. Drop elaboration
propagates that destructor across `Assign`/`Copy`/`Move` and emits it only when
`compute_dealloc_safe_locals` still proves the final pair is owned. The helper
loads the captures pointer, NULL-guards each eligible field in reverse capture
order, runs its user `def drop` plus type-directed free, then frees the captures
block and pair. Synthetic `__block`/`&block` parameters are call-duration
borrows, matching the rule that function parameters are not dropped by the
callee.

## How it avoids drop-after-move (the load-bearing soundness argument)

Drop emission is gated on a forward dataflow analysis,
`compute_dealloc_safe_locals` (`drops.rs:544`), which yields the set of locals
this frame *exclusively owns a fresh allocation for*. A local is excluded
("tainted") — and therefore **not** dropped — when any of the following hold:

- its value was passed as a `Use(local)` arg to any `Call`/`CallIndirect`
  (ownership may have transferred to the callee),
- it was stored into another aggregate via `SetField`,
- its pointer was aliased to another local via `Assign`/`Copy`/`Move` (dealloc
  responsibility moves to the last local in the chain — prevents double-free of
  a shared pointer, since MIR assignment is a pointer-copy, not a deep copy),
- it flows transitively into a `Return` value (`compute_return_alias_chain`,
  `drops.rs:760`) — the caller owns the returned heap, D4/D10,
- it was moved into an FFI-owning runtime structure
  (`runtime_abi::is_move_by_ffi`, e.g. `Task.spawn_raw(fut)` hands the future to
  the executor's queue).

The user-destructor call (step 1) is *additionally* gated on the same
`dealloc_safe` set, so a moved-from value's destructor never runs either.
Net invariant (matching tier1_04 §7.3): every non-Copy local that is owned at a
scope exit gets exactly one drop; a local that is moved/returned on that path
gets none.

## What is covered (proven green on this tree)

`cargo test -p ruxen_core --test drop_fixtures --test user_drop_runs` →
**19/19 pass** (cached: `tmp/test-cache/drop-suite-baseline.log`). The
`drop_fixtures` harness compiles each fixture against an allocation-tracking
runtime clone (wraps `ruxen_alloc`/`ruxen_dealloc` and raw `malloc`/`free` with
counters + an `atexit` leak report) and asserts `outstanding == 0`. Coverage:

- single Class/Struct heap locals at normal scope exit (the prompt's target slice):
  `runtime_no_leak_fixture` — `outstanding == 0`, `allocs >= 2`.
- user `def drop` runs exactly once at scope exit: `user_drop_runs.rs`,
  e2e case `134_user_drop_runs`.
- rebind frees the prior allocation (no leak): `reassignment_does_not_leak…`.
- loop-body locals across iterations + `break` + `continue` early-exit paths.
- `String` / `Array` / `Map` / `Set` locals incl. element-aware drops and
  ownership-transfer (no double-free): the `p07_*`, `p02b2_*`, `p03b2_*`,
  `p04b2_*` fixtures.
- move-by-FFI must NOT drop: `task_spawn_ownership.rs` +
  `task_spawn_move_by_ffi.rx` (the pin's own note: without the taint this
  SIGSEGVs on the first executor pump).
- e2e behavioural drop pins: `138_struct_drop_no_leak`, `518_file_drop_closes`
  (1024 fds without EMFILE), `545_tcp_listener_drop_closes`,
  `711_sharedsync_clone_drop_refcount`.
- closure capture glue: `closure_and_block_captures_are_freed_at_closure_drop`
  balances all three allocations for both closure syntaxes; the moved-out pin
  proves capture escape suppresses only the field free. Release-e2e 475 covers
  both surface forms through the normal compiler/runtime path.

## What this does NOT yet cover (staged remainder — open work)

- **Drop-flag elaboration for conditionally-moved locals.** Today a local moved
  on *some but not all* paths is classified by the conservative
  `dealloc_safe`/taint analysis, which biases toward *not* freeing (sound but
  may leak). Rust-style per-path drop flags (`MaybeDropped` in tier1_04 §7.2.2)
  are not implemented. Soundness is preserved (no double-free); the cost is a
  bounded leak on the not-moved path. This is the principled next increment.
- **Drop-on-unwind.** `panic = "abort"` only (tier1_04 §9 / NG1). No landing
  pads on either backend; drops do not run on panic. Blocked on a panic-strategy
  RFC.
- **Generic `T: Drop` monomorphisation checks** and **Copy ⊕ Drop mutual
  exclusion** (tier1_04 §4.6 / §5) — the `Copy`/`Clone` half of the tier-1.04
  feature, tracked separately.
- **Partial-move field-granular drop** (D6) beyond what the alias/taint analysis
  already handles structurally.
- **Precise closure capture drop flags.** A direct by-value capture is dropped
  only while its extracted MIR alias never returns, enters an aggregate, moves,
  or reaches a consuming/ambiguous call. Nested re-captures are not assigned a
  second owner. These conservative cases can leak until path-sensitive capture
  ownership is represented; they cannot double-free the escaped value.
- **Temporaries** (partly done 2026-10-17): a compiler temporary that owns a
  fresh value (a built-in heap type, a plain class or struct, or a class with a
  `def drop`) and is used only inside its statement is dropped at the end of
  that statement. Each statement that creates locals ends in a `MirInst::Nop`
  marker (`lower_scoped_statement`); `insert_drops` sinks the temporary's drop
  to it when the temporary and every borrow of it are dead after the marker,
  and strips the markers afterwards. Drop elaboration runs once every body is
  lowered (`Lowerer::elaborate_drops`), so a call result is owned by the
  caller when the callee returns a fresh value (`drops::returns_fresh`: every
  return is an allocation its own frame owned outright; a container only when
  its elements own no heap). An argument bound to a `&T` parameter the callee
  may return is no longer treated as a mere borrow. Still open: enum
  temporaries (a match moves a payload out without stripping the enum), a
  temporary whose statement spans several blocks (it keeps the
  function-exit drop), and results of functions that return a field or a
  parameter, which stay borrowed and are never freed.

## Notes for whoever picks up the remainder

- `MirInst::Drop` stays an inert marker. Do **not** add a `ruxen_dealloc` call in
  the backend Drop handler — the free is already emitted as a MIR `Call`; doing
  both double-frees. Both backend handlers carry this comment.
- Validate via the in-process harness (`drop_fixtures.rs` / `user_drop_runs.rs`
  / release-e2e), never via `ruxen compile` on a fixed `.rx` (the CLI caches by
  source hash and will not re-lower an unchanged file — see root `CLAUDE.md`).
- When widening the drop-eligibility filter in `drops.rs`, update the
  allocation-tracking splice in `drop_fixtures.rs` in the same change if a new
  free helper is introduced (it injects per-helper counters by header match).
