# Tier 3.07 — MIR Optimization Passes

Status: draft
Depends on: none
Blocks: nothing hard; improves Cranelift debug build output quality and LLVM IR compile time

---

## 1. Summary & motivation

Riven's MIR is produced by `crates/riven-core/src/mir/lower.rs` (3415
lines) and handed directly to codegen — Cranelift (`codegen/cranelift.rs`,
1127 lines) or LLVM (`codegen/llvm/emit.rs`, 1345 lines). There are **no
optimization passes between lowering and codegen**:

```
// mir/mod.rs
pub mod nodes;
pub mod lower;
#[cfg(test)]
mod tests;
```

(Three modules — no `optimize`, `const_fold`, `simplify_cfg`, `dce`, or
`inline`.)

This means:

- **Cranelift debug builds** get no optimization. LLVM's `optimize.rs`
  bails out early at `opt_level == 0` (`codegen/llvm/optimize.rs:13-15`).
  Cranelift never calls any optimizer. Unoptimized MIR flows straight
  through.
- **LLVM release builds** run LLVM's own `default<O2>` pipeline on the
  LLVM IR emitted from MIR. This catches most low-hanging fruit, but
  a simpler MIR means less IR to optimize, which means faster compile
  times and smaller IR.
- **Debuggability.** Unoptimized MIR is verbose (every sub-expression
  gets a temporary). Simplifying it before codegen produces cleaner
  LLVM IR when dumped with `--emit=mir` or `--emit=llvm-ir`.
- **Correctness testability.** Optimization passes are individually
  unit-testable in a way that "LLVM's O2" is not.

This doc specifies a MIR optimization pass pipeline: which passes ship,
where they run, how they compose, and which ones are v1 vs deferred.

---

## 2. Current state

### 2.1 MIR module structure

```
crates/riven-core/src/mir/
├── mod.rs        (4 lines — just re-exports)
├── lower.rs      (3415 lines — HIR → MIR)
├── nodes.rs      (367 lines — MIR AST)
└── tests.rs      (HIR → MIR round-trip tests)
```

No `optimize.rs`. No `passes/`. No pass-manager infrastructure.

### 2.2 MIR "optimization" inside lowering

The lowerer does some inlining of closure-taking methods:

- `try_inline_closure_method` (`lower.rs:2132-2249`) — inlines
  `.each`, `.filter`, `.find`, `.position`, `.map`, `.partition`
  closures directly at the call site, producing an inline loop.
- `inline_option_map` (`lower.rs:2888`) — inlines `Option.map` as a
  tag check.

These are not *optimization passes* in the traditional sense — they're
pattern-directed lowerings that avoid generating a closure call in the
first place. They produce already-optimized MIR, so subsequent opt
passes see less redundancy.

### 2.3 No CFG simplification, no constant folding, no DCE

Grep for `simplify_cfg`, `const_fold`, `const_prop`, `dce`, `dead_code`
under `crates/riven-core/src/mir/` — zero matches. The lowerer
produces unoptimized CFG, including:

- Empty basic blocks whose only purpose is to jump to another block
  (common near loop heads).
- Redundant temporaries (`let _t0 = 1; let _t1 = _t0; return _t1`).
- Dead assignments (`let _t0 = f(); let _t0 = g()` where the first is
  unused).
- Constant-foldable arithmetic (`2 + 3` emits `BinOp(Add, 2, 3)`
  instead of `5`).

### 2.4 LLVM opt pipeline

`codegen/llvm/optimize.rs:8-36` runs LLVM's `default<O2>` pipeline at
release mode; skips at `opt_level == 0`:

```rust
pub fn run_optimization(module: &Module, target_machine: &TargetMachine,
                        opt_level: u8) -> Result<(), String> {
    if opt_level == 0 {
        return Ok(());  // <-- early return at opt 0
    }
    // ... default<O2>, default<O3>, etc.
}
```

LLVM opts can clean up most of the fat in unoptimized MIR — but:
- They run on LLVM IR, so they pay the cost of emitting the fat IR first.
- They're skipped at opt 0 — so Cranelift-compiled debug builds have
  *no* optimizer.
- They don't exist for Cranelift release builds today (Cranelift's
  backend does some peephole optimization but nothing like LLVM's pass
  pipeline).

### 2.5 Cranelift has its own local optimizations

`cranelift-codegen` runs peephole patterns during lowering (e.g. folds
small constants). Not exposed as a separate pass. Not configurable.
Not sufficient to replace MIR-level opts.

---

## 3. Goals & non-goals

### Goals

1. A pass-manager in `crates/riven-core/src/mir/` with a pluggable
   pipeline.
2. v1 passes: `const_fold`, `simplify_cfg`, `dce`, `copy_propagation`.
3. Passes run at all opt levels, but with different intensities
   (opt 0 = minimal; opt ≥1 = full).
4. `--emit=mir` prints the post-optimization MIR.
5. Every pass is independently unit-tested.
6. Pass pipeline is deterministic: same input MIR → same output MIR.
7. No regressions: all existing tests must pass.

### Non-goals

- **MIR-level inlining** (cross-function). v2. The lowerer already
  inlines closures at their call sites (§2.2), which gets most of
  the practical benefit.
- **Loop unrolling / vectorization.** These are LLVM's job.
- **Aggressive optimizations requiring whole-program info** (escape
  analysis, devirtualization). v2+.
- **GVN / CSE.** v2 — big enough to justify its own doc.
- **Register allocation hints.** Not MIR-level.
- **SROA (scalar replacement of aggregates).** v2.
- **Replacing LLVM's optimizer.** MIR opts complement LLVM, not
  replace it.

---

## 4. Surface

### 4.1 CLI interaction

No new user-facing flags. MIR opts run automatically based on
`--opt-level`:

| `--opt-level` | Passes run |
|---|---|
| 0 (default debug) | `simplify_cfg` only (lightweight; improves debug-info quality) |
| 1 | `simplify_cfg`, `const_fold`, `dce` |
| 2 (default release) | + `copy_propagation` |
| 3 | repeat the pipeline until fixed point |
| s/z | opt 2 + prefer-smaller variants |

Debug builds (`--debug` from doc 02) force `--opt-level=0` behavior on
the MIR side — even though LLVM might optimize differently.

An optional `--mir-opts=off` flag disables all MIR optimization for
comparison / debugging:

```
rivenc --opt-level=2 --mir-opts=off hello.rvn
```

### 4.2 Public API

```rust
// mir/optimize/mod.rs
pub struct PassManager {
    passes: Vec<Box<dyn MirPass>>,
}

impl PassManager {
    pub fn default(opt_level: u8) -> Self;
    pub fn add<P: MirPass + 'static>(&mut self, pass: P);
    pub fn run(&self, program: &mut MirProgram) -> Result<(), String>;
}

pub trait MirPass: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, program: &mut MirProgram) -> Result<(), String>;
}
```

### 4.3 Pass invocation in the compiler

`crates/riven-core/src/codegen/mod.rs::compile_with_options`
(`:86-129`) is the current boundary where MIR leaves and codegen
begins. Insert the pass manager there:

```rust
pub fn compile_with_options(
    program: &MirProgram,  // take &mut or clone()
    ...
) -> Result<(), String> {
    let mut program = program.clone();  // or accept &mut
    let pm = mir::optimize::PassManager::default(opt_level);
    pm.run(&mut program)?;
    // ... existing codegen continues
}
```

---

## 5. Architecture / design

### 5.1 Pass ordering

Order matters. Most passes fixpoint with each other; one pass's output
unblocks the next:

```
simplify_cfg         # remove empty blocks, merge linear chains
  → const_fold       # fold BinOp(Add, Int(2), Int(3)) → Assign(_t, Int(5))
    → dce            # remove Assign dests that aren't used
      → copy_prop    # propagate simple copies
        → simplify_cfg  # re-simplify after copy-prop
          → dce      # re-DCE after simplification
```

At opt-level ≥ 3, repeat the whole pipeline until a fixed-point (no
pass changes anything). Cap iterations at, say, 10 to avoid
pathological cases.

### 5.2 Pass: `simplify_cfg`

Remove trivially-removable control flow:

1. **Empty-block elimination.** A block with no instructions and a
   `Terminator::Goto(X)` can be replaced by its target in every
   predecessor's terminator.
2. **Block merging.** If block `A` ends with `Goto(B)` and `B` has only
   `A` as a predecessor, merge `B`'s instructions into `A` and take
   `B`'s terminator.
3. **Branch constant-folding.** `Terminator::Branch { cond: Literal(Bool(true)), then_block, .. }` → `Goto(then_block)`. Corresponding `else_block` may become unreachable.
4. **Unreachable-block removal.** After step 3, run a reachability
   pass (DFS from `entry_block`) and drop unreachable blocks.

Cost: O(|blocks| + |preds|). Run first, run cheaply, run at every
opt level including 0 (small improvement to debug info readability).

### 5.3 Pass: `const_fold`

Walk every instruction. For each `MirInst`:

- `BinOp` with both operands `MirValue::Literal(_)` → replace with
  `Assign { dest, value: Literal(computed) }`.
- `Compare` with literal operands → `Assign { dest, value: Literal(Bool(...))}`.
- `Negate` / `Not` on literals → fold.

Handle the integer-overflow rules: wrapping for unsigned, panic-on-
overflow at runtime for signed (match current MIR semantics — consult
`lower.rs` for the current contract).

Be careful with:

- Float semantics (NaN, signed zero). Use `f64::op` exactly as codegen
  does.
- String concat? — literal-string concat is a candidate but is
  probably better left to later (no current IR support for string
  const operands beyond `StringLiteral`). Skip v1.

### 5.4 Pass: `dce` (dead-code elimination)

Standard algorithm:

1. Build a **use-set**: all `LocalId`s that appear as operands in any
   instruction or terminator.
2. For each instruction that writes a `LocalId` not in the use-set:
   if the instruction has no side effects, remove it.
3. Iterate until no more removals.

Side-effectful instructions that must not be DCE'd: `Call`, `Drop`,
`StringLiteral` (allocates), `Alloc`, `StackAlloc`. Conservative for
v1: skip DCE for any `MirInst::Call` — v2 adds a "pure function"
annotation for DCE-safe calls.

### 5.5 Pass: `copy_propagation`

For every `Assign { dest: X, value: Use(Y) }`, replace subsequent uses
of `X` with uses of `Y` within the current block (intraprocedural,
per-block). Then DCE picks up the now-dead `X` assignment.

Cross-block copy propagation is SSA-shaped work — defer to v2 when
we introduce an SSA form for MIR (not a current plan).

### 5.6 Pass: `branch_threading` (stretch)

If block A's only successor is B, and B starts with a `Switch` on a
value that's constant at A's end, replace B's terminator at A with
the switch's resolved target. Cheap and helpful; v2.

### 5.7 Pass manager interactions with opt levels

```rust
impl PassManager {
    pub fn default(opt_level: u8) -> Self {
        let mut pm = PassManager { passes: vec![] };
        pm.add(SimplifyCfg);
        if opt_level >= 1 {
            pm.add(ConstFold);
            pm.add(Dce);
        }
        if opt_level >= 2 {
            pm.add(CopyProp);
            pm.add(SimplifyCfg);  // re-run
            pm.add(Dce);          // re-run
        }
        if opt_level >= 3 {
            pm.fixpoint = true;
        }
        pm
    }
}
```

### 5.8 Determinism

Every pass must be deterministic. Tests:

- Run the pipeline twice on the same input; the output MIR must be
  byte-identical (via `Debug` formatting of MirProgram).
- Pass ordering must be deterministic — no `HashMap<T, _>` iteration
  where ordering affects output.

### 5.9 Interaction with `--emit=mir`

Currently `--emit=mir` prints the post-lowering MIR
(`rivenc/src/main.rs:662-682`). After this doc lands, it prints the
**post-optimization** MIR. Add `--emit=mir-raw` for the pre-opt view,
useful for debugging pass correctness.

### 5.10 Interaction with debug info (doc 02)

MIR opts erase information. A `let x = 42` optimized out by DCE leaves
no `x` for the debugger. Standard behavior — matches rustc.

Key correctness constraint: if `--debug` is set, `dce` must preserve
*any* local for which debug info would have been emitted. Approach:
before running `dce`, mark every named (non-temporary) local as
"preserved" and skip them. Temporaries (names starting with `_t`) are
still eligible.

---

## 6. Implementation plan

### Files to touch

| Phase | File | Change |
|---|---|---|
| 1 | `crates/riven-core/src/mir/mod.rs:1-4` | Export `pub mod optimize` |
| 1 | `crates/riven-core/src/mir/optimize/mod.rs` *new* | `PassManager` + `MirPass` trait |
| 1 | `crates/riven-core/src/mir/optimize/simplify_cfg.rs` *new* | ~200 lines |
| 2 | `crates/riven-core/src/mir/optimize/const_fold.rs` *new* | ~150 lines |
| 2 | `crates/riven-core/src/mir/optimize/dce.rs` *new* | ~150 lines |
| 3 | `crates/riven-core/src/mir/optimize/copy_prop.rs` *new* | ~100 lines |
| 4 | `crates/riven-core/src/codegen/mod.rs:86-129` | Call pass manager |
| 4 | `crates/rivenc/src/main.rs` | Add `--mir-opts=off`; add `--emit=mir-raw` |
| 5 | `crates/riven-core/src/mir/optimize/tests.rs` *new* | Per-pass unit tests |

### Phase breakdown

**Phase 1 — Pass manager + simplify_cfg (3 days).**
- Day 1: `PassManager`, `MirPass` trait, integration into
  `codegen::compile_with_options`.
- Day 2-3: `simplify_cfg` pass with tests. Run existing compiler tests
  to confirm no regressions.

**Phase 2 — const_fold + dce (3 days).**
- Day 1-2: `const_fold` with extensive arithmetic tests.
- Day 3: `dce` with side-effect conservatism.

**Phase 3 — copy_prop + pipeline integration (2 days).**
- Day 1: `copy_prop`.
- Day 2: Opt-level dispatch; fixpoint iteration at opt 3.

**Phase 4 — Debug-info interaction (1 day).**
Preserve named locals under `--debug`.

**Phase 5 — Benchmarks and CI (1 day).**
Measure compile-time impact on a medium project. Ensure opt passes
don't dominate compile time.

Total: ~10 engineer-days.

---

## 7. Interactions with other tier-3 items

- **Doc 02 (debugger).** DCE/const-fold erase debug info. Must respect
  the `--debug` flag (§5.10).
- **Doc 05 (bench).** `black_box` must be opaque to all MIR opts.
  `const_fold` must treat `black_box(5)` as non-constant; `dce` must
  not eliminate a `black_box` call. Add a dedicated regression test.
- **Doc 06 (incremental).** MIR opts become a new query
  `mir_optimized(fn_id)` depending on `mir(fn_id)`. Cache-friendly.
- **Doc 03 (test).** Assertions with constant folding (`assert_eq(1, 1)`)
  should compile-warn. Stretch goal.
- **Doc 01 (LSP).** MIR emitted to `--emit=mir` in LSP code-lens
  (v2) should be the post-opt view.

### Tier-1 dependencies

None.

---

## 8. Phasing

| Phase | Scope | Days |
|---|---|---|
| 1 | Pass manager + simplify_cfg | 3 |
| 2 | const_fold + dce | 3 |
| 3 | copy_prop + pipeline | 2 |
| 4 | Debug-info preservation | 1 |
| 5 | Benches + CI | 1 |

---

## 9. Open questions & risks

1. **OQ-1 — Arithmetic overflow in const_fold.**
   Should `Int(i64::MAX) + Int(1)` fold to a runtime-panicking
   expression (preserve the panic) or wrap (hide the bug)? The
   existing runtime panics on signed overflow at runtime; const_fold
   must produce the same result. Recommendation: fold to a
   compile-error diagnostic when both operands are known literals
   and would overflow. This is more aggressive than rustc's default
   but aligns with P1 (safety).
2. **OQ-2 — When to run MIR opts if LLVM will O2 anyway.**
   Some passes (like dce) duplicate LLVM's work. Argument for running
   them anyway: (a) speeds up LLVM by shrinking input, (b) improves
   Cranelift output where LLVM isn't involved. Validate with
   before/after benchmarks on a medium project.
3. **OQ-3 — Pass ordering locked or user-configurable.**
   rustc lets internal pass ordering be scripted via `-Z`. Do we
   expose ordering? v1: locked. v2: `--mir-opts=simplify,constfold,dce`
   style flag.
4. **OQ-4 — Effect tracking for DCE.**
   `MirInst::Call` is conservative (never removed). This misses
   opportunities. v2: annotate `FnSignature` with an `is_pure: bool`;
   DCE removes pure calls whose results are unused. Blocked on some
   effect-analysis work.
5. **R1 — Pass bugs cause miscompiles.**
   A bad DCE removes a live instruction. Differential testing: run
   the test suite at opt-level 0 AND opt-level 2 and assert identical
   program output. CI should gate on this.
6. **R2 — Fixpoint can loop.**
   Cap iterations at 10; panic if exceeded. Usually a pass bug.
7. **R3 — Compile-time regression.**
   Adding opt passes slows the compiler. Bench the cold and warm
   compile times of a medium project. If MIR opts add >5% to cold
   compile, revisit.
8. **OQ-5 — Pattern-specific passes.**
   E.g. `enum_tag_elision` — if a discriminant is only written and
   never read, skip the tag. Worth it? Measure with real programs.
9. **R4 — Interaction with future SSA-shaped passes.**
   Adding SSA would let us do GVN/CSE cheaply. v1 passes are
   block-local to avoid SSA. Doesn't preclude SSA v2 — the passes
   just become more capable.
10. **OQ-6 — Cranelift peephole already does some of this.**
    Some of the wins from `simplify_cfg` land in Cranelift's own
    passes. Measure. If Cranelift handles 80% of it,
    `simplify_cfg` is still valuable for LLVM IR size.
11. **OQ-7 — Should we add a MIR verifier?**
    A verifier walks the CFG and asserts invariants (every block has
    a terminator; Goto targets exist; etc.). Highly recommended
    alongside opt passes — catches bugs early. Ship as Phase 0.

---

## 10. Test matrix

Per-pass unit tests, each taking small hand-written MIR input:

| Pass | Test | Input → Output |
|---|---|---|
| simplify_cfg | empty block | `bb0: goto bb1; bb1: goto bb2; bb2: return` → `bb0: return` |
| simplify_cfg | constant branch | `branch(true, bb1, bb2)` → `goto bb1` |
| simplify_cfg | unreachable removal | After constant-branch, old bb2 gone |
| const_fold | add | `_t = 2 + 3` → `_t = 5` |
| const_fold | compare | `_t = 5 < 10` → `_t = true` |
| const_fold | no-op on var | `_t = x + 3` unchanged |
| dce | dead assign | `_t = 42; return` → `return` |
| dce | side-effect preserved | `call puts("hi"); return` unchanged |
| dce | debug preserves named | With `--debug`, `let x = 42; return` keeps x |
| copy_prop | direct | `_t = x; _u = _t + 1` → `_u = x + 1` |
| interaction | const + dce | `_t = 2 + 3; _u = _t + 1; return _u` → `return 6` (after fold + prop + dce) |

Integration tests:

| Case | Assertion |
|---|---|
| All existing tests pass at opt 0 | No regression |
| All existing tests pass at opt 2 | No regression |
| Differential opt-0 vs opt-2 | Same stdout |
| `--mir-opts=off` disables all passes | `--emit=mir` output identical to today |
| `--emit=mir-raw` shows pre-opt MIR | Documented state preserved |
| Benchmark: compile medium project at opt 2 | <5% regression |
| `black_box` not folded | `_t = black_box(5) + 0` stays as-is |

Add a MIR verifier and run it after every pass in debug mode.
