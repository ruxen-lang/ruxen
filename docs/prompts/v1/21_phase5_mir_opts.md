# 21 — Phase 5: MIR optimizations (T3.07)

**Depends on:** prompt 11 (incremental, so opts can be cached).
**Reads:** `docs/requirements/tier3_07_mir_optimizations.md`.

## Scope (v1)

Conservative passes only. Aggressive opts are LLVM's job.

Implement, in order:

1. **`simplify_cfg`** — merge linear blocks, drop unreachable.
   Always-on, even at opt level 0.
2. **`dead_local_elim`** — locals whose only def is unused.
3. **`const_prop`** — propagate `Literal(...)` values through
   `Assign`/`BinOp`/`Compare` when both sides are literals.
4. **`copy_prop`** — collapse `t = x; y = t` into `y = x` when `t`
   has a single use.
5. **`branch_fold`** — collapse `if true { A } else { B }` to `A`.

## TDD

For each pass:
- Unit test in `crates/riven-core/src/mir/opts/`: hand-build a MIR
  fn, run the pass, assert structure of output.
- Snapshot test: dump MIR before/after on a fixture program; assert
  expected savings (instruction count drop).
- Bench: aggregate compile-time should NOT regress >2% with all
  passes on at opt level 1.

## Implementation

- Each pass is `fn pass_name(func: &mut MirFunction)`.
- Driver in `mir/opts/mod.rs` runs passes in order.
- Opt level controlled via CLI `-O0` / `-O1` (default 1 for
  release, 0 for debug).
- Passes idempotent — re-running yields same result.

## Definition of done

- [ ] Five passes implemented with unit + snapshot + bench tests.
- [ ] Compile-time impact <2% at opt level 1.
- [ ] Generated code measurably smaller on fixtures (count
      `MirInst` reduction).
- [ ] CHANGELOG bullet.
