# Tier 3.05 — Benchmarking (`@[bench]` + `riven bench`)

Status: draft
Depends on: doc 03 (test framework) — shares 90% of the harness
Blocks: regression tracking of compiler performance (as a special application)

---

## 1. Summary & motivation

Riven has no first-class benchmarking surface. Users cannot write
`@[bench] def measure_x ... end` and run `riven bench` to get
statistical measurements. `criterion` is used for Rust-level
benchmarks inside the compiler (`rivenc/benches/cache_bench.rs` + the
`criterion = "0.5"` dev-dep in `rivenc/Cargo.toml:22`), but that
benchmarks the compiler, not user programs.

This doc specifies v1 benchmarking: attribute, subcommand, harness,
statistics, regression tracking. It is intentionally scoped tightly —
everything here reuses the test-framework plumbing from doc 03.

---

## 2. Current state

### 2.1 No `@[bench]` attribute

Same state as `@[test]`. Parser attribute dispatch at
`crates/riven-core/src/parser/mod.rs:473-511` recognizes only `link`
and `repr`. `@[bench]` above a `def` errors out.

### 2.2 No `riven bench` subcommand

`crates/riven-cli/src/cli.rs:25-114` has no `Bench` variant.

### 2.3 No harness

No statistical-measurement code anywhere in the workspace. Nothing
measures per-iteration time, warmup, outliers, stddev, or regression.

### 2.4 `criterion` is a known-good pattern to imitate

`rivenc/benches/cache_bench.rs:10` shows the compile-time usage:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
```

We can't pull `criterion` *into* user Riven programs (it's a Rust
crate), but we can copy its statistical approach: warmup, iteration
count auto-scaling, mean + stddev, outlier detection via MAD
(median absolute deviation).

---

## 3. Goals & non-goals

### Goals

1. Write a benchmark in Riven with `@[bench]` above a `def`.
2. `riven bench` builds and runs all benchmarks, reports wall time +
   stddev + throughput.
3. Auto-scaling iteration count so that even fast benchmarks produce
   meaningful statistics.
4. `black_box` primitive so the optimizer can't eliminate the work.
5. Filter by name (same semantics as `riven test`).
6. JSON output for CI and regression tools.
7. Optional baseline comparison: save a result, compare a later run.

### Non-goals

- **Statistical rigor beyond mean + stddev + MAD.** No bootstrap
  confidence intervals, no t-tests. v2.
- **Memory benchmarking.** Allocation counting, peak RSS, etc. v2.
- **Hardware-performance-counter support** (perf events). v2.
- **Multithreaded bench.** Blocked on tier-1 doc 02.
- **CPU-pinning / isolation.** Out of scope; document what matters
  and let the user configure OS-level isolation.
- **Criterion-compatibility** beyond "we can export a JSON format
  their tooling consumes." Not a tight bind.

---

## 4. Surface

### 4.1 Attribute

```
## Measures the cost of allocating a Vec of 1000 ints.
@[bench]
def alloc_vec(ctx: &mut Bencher)
  ctx.iter do
    let mut v = Vec.new()
    for i in 0..1000
      v.push(i)
    end
    black_box(v)
  end
end
```

Variants:

```
@[bench]                              # standard
@[bench(skip)]                        # discovered but not run
@[bench(skip = "too slow for CI")]
@[bench(warmup = 100)]                # override warmup iterations
@[bench(sample_size = 1000)]          # override sample count
```

### 4.2 CLI

```
riven bench                          # build + run all benches
riven bench --filter="foo"
riven bench --format=pretty|json|csv
riven bench --save-baseline=master
riven bench --baseline=master        # compare to saved baseline
riven bench --list                   # just enumerate benches
riven bench --sample-size=500
riven bench --warmup=50
riven bench --release                # always on for bench; explicit for clarity
riven bench --no-run                 # build but don't execute
riven bench --bin foo                # limit to a specific binary
```

### 4.3 `Bencher` type and `black_box`

```
# In std::test::bench (same module as @[test] helpers)

class Bencher
  pub def iter[F](f: F) where F: Fn -> Unit
    # Internally: warmup + sample loop + timing
  end

  pub def iter_with_setup[F, G, S](setup: G, run: F)
    where G: Fn -> S, F: Fn(S) -> Unit
    # Amortize setup cost
  end
end

pub def black_box[T](value: T) -> T
  # Opaque to the optimizer — implemented as a volatile asm intrinsic.
end
```

### 4.4 Output format (pretty)

```
running 3 benches
bench tests::vec::alloc_1000           ...   1.24 µs ± 0.03 µs  (804 Melem/s)  [n=500]
bench tests::hash::insert_1000         ...  12.31 µs ± 0.22 µs  ( 81 Melem/s)  [n=500]
bench tests::sort::quicksort_random    ... 142.50 µs ± 3.10 µs  (  7 Melem/s)  [n=200]

bench result: 3 ran, 0 skipped; finished in 3.72s
```

JSON format: one record per benchmark:

```json
{
  "name": "tests::vec::alloc_1000",
  "mean_ns": 1240.3,
  "stddev_ns": 31.2,
  "median_ns": 1230.0,
  "min_ns": 1210.0,
  "max_ns": 1450.0,
  "mad_ns": 12.1,
  "sample_size": 500,
  "throughput": { "unit": "elem", "per_iter": 1000 }
}
```

### 4.5 Baseline comparison

```
# On master:
$ riven bench --save-baseline=master
# ... runs benches, saves results to target/bench/baselines/master.json

# After change:
$ riven bench --baseline=master
bench tests::vec::alloc_1000   ...   1.38 µs ± 0.04 µs  [n=500]  +11.2% (regressed)
bench tests::hash::insert_1000 ...  12.12 µs ± 0.20 µs  [n=500]   -1.5% (noise)
bench tests::sort::quicksort   ... 142.01 µs ± 3.00 µs  [n=200]  -0.3% (unchanged)
```

Regression threshold: difference outside mean ± 3× stddev of combined
samples. Tunable via `--significance=n` flag.

---

## 5. Architecture / design

### 5.1 Reuse from doc 03 (test framework)

Every §5.x section in doc 03 applies here with renaming:

- Attribute-pipeline work from doc 03 Phase 1 is shared. `@[test]` and
  `@[bench]` both need `attributes: Vec<Attribute>` on `FuncDef`.
- Discovery walks the same tree (`src/**.rvn` + `benches/**.rvn` ++
  `tests/**.rvn`; see §5.3 for where benches live).
- Harness generation is the same pattern: collect, synth a `main`,
  link.
- Filter/list/format semantics match.

New bits specific to bench:

- The `Bencher` type and `iter` method — lives in `std::test::bench`.
- The `black_box` intrinsic (§5.5).
- Statistical analysis (§5.6).
- Baseline storage + comparison (§5.7).

### 5.2 Harness generation

Benches also run fork-per-bench (recommended) to isolate memory and
avoid one crash killing the whole run. Each child:

1. Receives its bench name + config on stdin.
2. Calls the bench function with a `Bencher`.
3. Writes measurements to stdout as JSON.
4. Exits.

Parent collects and aggregates.

### 5.3 Where benches live

Three options:

- **`benches/**.rvn`** — dedicated directory (Rust/Cargo convention).
- **Inline with `@[bench]`** in `src/**.rvn`.
- **Inline with `@[bench]` in `tests/**.rvn`** — co-locate perf regression
  tests with correctness tests.

Support all three. `riven bench` picks up benches anywhere in `src/`,
`tests/`, or `benches/`. Paths:

- `src/foo.rvn` `@[bench] def b` → `foo::b`
- `tests/foo.rvn` `@[bench] def b` → `tests::foo::b`
- `benches/foo.rvn` `@[bench] def b` → `benches::foo::b`

### 5.4 Timing measurement

Use the clock primitive exposed by tier-1 stdlib:
`std::time::Instant::now() -> Instant`, `Instant.elapsed() -> Duration`,
`Duration.as_nanos() -> UInt64`. If stdlib hasn't landed, ship a
minimal C-level wrapper in the runtime for the bench harness only:
`riven_bench_now_ns() -> UInt64` using `clock_gettime(CLOCK_MONOTONIC)`.

### 5.5 `black_box` intrinsic

LLVM provides `llvm.donothing` and inline assembly tricks to prevent
DCE. Simplest portable implementation: volatile asm.

In `codegen/llvm/emit.rs` and `codegen/cranelift.rs`, special-case
calls to the `black_box` builtin by emitting:

- LLVM: `call void asm sideeffect "", "r,~{memory}"(i64 %val)` — forces
  the optimizer to treat `val` as having an observable effect.
- Cranelift: insert a `ir::Opcode::Nop` with a side-effect flag, or
  an explicit volatile load/store of `val`.

Reference: how `std::hint::black_box` works in Rust. Don't reinvent
the details.

### 5.6 Sampling algorithm

Adapted from criterion's default algorithm, minus the bootstrap:

1. **Warmup phase** — run the bench closure for a fixed wall-clock
   duration (default 1.0 s) to let caches settle and JIT code paths
   warm up.
2. **Measurement phase** — run the closure for another fixed duration
   (default 3.0 s), recording per-iteration times. Auto-scale:
   if a single iteration is ≥10 µs, record each individually.
   Otherwise batch `N` iterations per sample, where `N` is chosen so
   each sample is ≥10 µs (reduces clock overhead).
3. **Analysis** — compute mean, median, stddev, min, max, MAD. Discard
   outliers beyond 3× MAD from median for the "clean" statistics.

Target sample size: 100-500 samples. Lower-bound: if 30 samples can't
fit in 3 seconds, the benchmark is very slow; warn the user.

### 5.7 Baseline storage

Directory: `target/bench/baselines/<name>.json`. File format: array of
the JSON records from §4.4. On `--baseline=foo`, load
`target/bench/baselines/foo.json` and diff the current run against it
by name.

Versioning: include compiler version + git SHA in the baseline header
so stale comparisons can warn.

### 5.8 Release-mode default

Benches always build in release mode by default. `--no-release` flag
overrides (for debugging a bench's correctness). Release is chosen
because debug-mode benches are usually uninformative (see comparable
Rust behavior).

---

## 6. Implementation plan

### Files to touch

| Phase | File | Change |
|---|---|---|
| 1 | *shared with doc 03* | Attribute pipeline for `@[bench]` — same change as `@[test]`; if doc 03 lands first, this is a one-line attribute-name addition |
| 2 | `crates/riven-cli/src/cli.rs:25-114` | Add `Bench` command |
| 2 | `crates/riven-cli/src/bench.rs` *new* | Parallel to `test.rs`; ~300 lines |
| 2 | `crates/riven-cli/src/main.rs` | Wire `Command::Bench` |
| 2 | `crates/riven-cli/src/test_harness.rs` *new* (refactor) | Shared discovery + build logic between test and bench |
| 3 | `stdlib/test/bench.rvn` *new* | `Bencher` class + `iter` method |
| 3 | `crates/riven-core/src/resolve/mod.rs` | Register `black_box` builtin |
| 3 | `crates/riven-core/src/codegen/llvm/emit.rs` | Emit volatile asm for `black_box` calls |
| 3 | `crates/riven-core/src/codegen/cranelift.rs` | Same for Cranelift |
| 4 | `crates/riven-core/runtime/runtime.c` | Add `riven_bench_now_ns` if tier-1 stdlib hasn't shipped |
| 4 | `crates/riven-cli/src/bench.rs` | Sampling + stats + comparison |
| 5 | Docs, scaffold, distribution | `riven new` adds a `benches/` example |

### Phase breakdown

**Phase 1 — Attribute plumbing (0.5 day).**
If doc 03 has landed: add `"bench"` to the allowed attribute names.
If not: do the same AST/HIR work doc 03 describes for `@[test]` at
the same time as `@[bench]`.

**Phase 2 — CLI + harness (3 days).**
- Day 1: `riven bench` subcommand; discovery walks `benches/**.rvn`
  in addition to the test paths.
- Day 2: harness gen (fork-per-bench); stdin/stdout JSON protocol.
- Day 3: filter, list, --release default, --no-run.

**Phase 3 — Statistics + `black_box` (2 days).**
- Day 1: sampling algorithm + mean/stddev/MAD.
- Day 2: `black_box` codegen special case (LLVM + Cranelift).

**Phase 4 — Baselines + output formats (1 day).**
Save/load `target/bench/baselines/<name>.json`; pretty + JSON + CSV.

**Phase 5 — Polish (0.5 day).**
Scaffold benches example; docs; warn-on-too-few-samples.

Total: ~7 engineer-days after doc 03.

---

## 7. Interactions with other tier-3 items

- **Doc 03 (test).** Shared attribute pipeline and harness. Extract
  shared code into `crates/riven-cli/src/test_harness.rs`.
- **Doc 07 (MIR opts).** `black_box` correctness must be preserved
  through any MIR optimization. Any new opt pass must not see through
  `black_box`. Write an integration test.
- **Doc 06 (incremental).** Cache bench objects same as test objects
  with `flags="bench"`.
- **Doc 02 (debugger).** `riven bench --debug` not meaningful (debug
  builds are slow by design). Document this and skip.
- **Doc 04 (doc generator).** `rivendoc` can link to bench results
  per-function. v2.

### Tier-1 dependencies

- **Tier-1 doc 01 (stdlib).** `std::time::Instant` is the clock source.
  Until it exists, `riven_bench_now_ns` ships as a bench-only runtime
  helper.
- **Tier-1 doc 04 (Drop).** Leaking benches are fine for fork-per-bench
  runs (child exits, OS reclaims) but allocate-hot benches will skew
  results without Drop. Document the caveat.
- **Tier-1 doc 05 (derive/macros).** Doesn't block v1.

---

## 8. Phasing

| Phase | Scope | Days | Prereqs |
|---|---|---|---|
| 1 | Attribute plumbing | 0.5 | Doc 03 Phase 1 |
| 2 | CLI + harness | 3 | 1 |
| 3 | Stats + `black_box` | 2 | 2 |
| 4 | Baselines + formats | 1 | 3 |
| 5 | Polish | 0.5 | 4 |

---

## 9. Open questions & risks

1. **OQ-1 — `Bencher::iter` closure API.**
   Crystal-style `do |ctx| ... end` or Rust-style `ctx.iter do ... end`?
   Recommend the latter — matches the pattern users will write in
   tests.
2. **OQ-2 — Auto-scaling iteration count semantics.**
   If a benchmark runs in 100 ns, a single sample records ~50 µs of
   iterations (500×). Is that the right scale? Match criterion's
   default (~50 ms per sample in adaptive mode). Phase 3 decision.
3. **OQ-3 — How to compare across different benchmark count scales.**
   If baseline had 500 samples and current has 200, is the comparison
   valid? Recommend: always use the min of the two sample counts and
   warn the user.
4. **OQ-4 — Significance threshold default.**
   Recommend 3× combined stddev. Too sensitive → noise. Too loose →
   regressions slip through. Tunable per-project.
5. **OQ-5 — CSV columns.**
   Freeform or canonical? Recommend: `name,mean_ns,stddev_ns,n,change_pct`.
   Other statistics come from JSON.
6. **R1 — Cranelift `black_box` correctness.**
   Cranelift has no built-in volatile-asm primitive. May need to
   emit a volatile load/store of `value` through a stack slot. Test
   that `let x = black_box(10); x + 0` does not fold.
7. **R2 — Clock precision.**
   `clock_gettime(CLOCK_MONOTONIC)` has ~1 ns resolution on modern
   Linux; on macOS via `mach_absolute_time`, similar. Windows: out
   of scope.
8. **R3 — OS noise.**
   Per-process scheduling jitter can add 10-100 µs to individual
   samples. Document that benchmarks should be run on a quiet system;
   MAD-based outlier rejection handles most of this.
9. **OQ-6 — Baseline name.**
   Arbitrary string vs constrained to `[a-z0-9_-]*`. Recommend
   constrained (sanitize for filesystem use).
10. **R4 — Build-cache invalidation on flag changes.**
    Different `--sample-size` shouldn't rebuild the bench binary — it's
    a runtime argument. Verify.
11. **OQ-7 — Throughput annotations.**
    Author writes `@[bench(throughput = "1000 elem")]` to get
    Melem/s reporting? Or compute from a `ctx.throughput(Throughput::Elements(1000))`
    call inside the bench body? Recommend the latter (matches
    criterion; stays on the Riven side, not the attribute side).

---

## 10. Test matrix

| Case | Assertion |
|---|---|
| `@[bench] def b(ctx: &mut Bencher)` compiles | No parse/type error |
| `@[bench] def b` without ctx param | Compile error: benches need `&mut Bencher` param |
| `riven bench` runs all benches | Exit 0, output lists them |
| `riven bench --filter=foo` | Only matching benches run |
| `riven bench --list` | No execution, just names |
| Baseline save/load round-trip | Comparison shows 0% diff |
| `black_box(expr)` preserved through MIR/LLVM | Bench scales with input, not folded away |
| Bench with `ctx.iter do ... end` | Measurements produced |
| Very fast bench (<100 ns/iter) | Auto-scales sample size, reports sensibly |
| Very slow bench (>1s/iter) | Reports with small sample size + warning |
| Bench that panics | Reported as failed, others continue |
| `--sample-size=100` | Respected |
| `--no-run` | Binary built, not executed |
| JSON output parseable | Validates against the §4.4 schema |
| CSV output | Exactly the columns listed in OQ-5 |
| Cranelift backend bench | `black_box` still works |
| Baseline mismatch warning | When compiler version changes, baseline carries old version |
