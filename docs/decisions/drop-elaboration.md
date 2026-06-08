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
   - `Ty::Array(elem)` → element-aware `ruxen_vec_drop_string` /
     `ruxen_vec_drop_vec`, else spine-only `ruxen_vec_free`
   - `Ty::Map(k,v)` / `Ty::Set(elem)` → element-aware `ruxen_hash_drop_*` /
     `ruxen_set_drop_string`, else spine-only `ruxen_hash_free` / `ruxen_set_free`
   - `Ty::Class/Struct/Enum` → `ruxen_dealloc`
   - any other type reaching this point → `unimplemented!` (loud, not a silent
     fall-through), since the eligibility filter restricts to exactly these.

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
- **Temporaries**: the current filter drops only `_t*` temps that are built-in
  heap types proven alloc-rooted; the general "drop every non-Copy temp at
  end-of-statement" rule (tier1_04 §7.2.6) is not in place.

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
