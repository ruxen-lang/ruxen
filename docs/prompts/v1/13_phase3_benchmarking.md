# 13 — Phase 3: benchmarking (T3.05)

> **Status: ✅ Shipped** (audited 2026-05-21). Bencher harness in
> `library/std/bench/` (pure Riven `Bencher` class with auto-scaling
> `iter` + 5-LOC C `black_box` opaque-identity shim). `rivenc bench
> <file.rvn>` CLI subcommand with `--filter` + `--iter-hint` flags
> in `src/rivenc/src/main.rs`. Five criterion compile-pipeline
> benches in `src/rivenc/benches/{parse,resolve,typeck,mir,codegen}_bench.rs`
> against the 508 + 727 real fixtures. Bench fns are identified by
> name convention (`def bench_*(b: &var Bencher)`) — no compiler
> changes per `feedback_pure_riven_first.md`. **Deferred to v1.5**
> per spec's "optional" list: JSON output, baseline comparison,
> MAD outlier detection, the `bench` in-body parser directive.

**Depends on:** prompt 11.
**Reads:** `docs/requirements/tier3_05_benchmarking.md`.

## Goal

`riven bench` runs functions annotated with a `bench` directive in a
project and reports per-iteration timing.

## Surface

```riven
def bench_string_concat(b: &var Bencher)
  bench
  b.iter || -> {
    var s = String.new
    for i in 0..100
      s.push_str("xx")
    end
    s
  }
end
```

`bench` is an in-body directive (same pattern as `include`,
`inline :name`, `deprecated`). When the directive
appears in a `def` body, the function is collected by the bench
runner.

## TDD

- Unit test: `Bencher.iter(closure)` runs the closure ≥ N times,
  reports min/median/p99 in ns.
- Integration test: a fixture project with two `bench`-marked fns
  runs via `riven bench`, parses output, asserts timings reported.

## Implementation

- Add `crates/riven-bench/` (or fold into `riven-cli`).
- `Bencher` runtime tracks iterations, uses `clock_gettime`
  monotonic clock.
- Auto-tunes iteration count for ≥100ms total run (criterion-style).
- CLI flags: `--filter <pat>`, `--iterations N`, `--baseline FILE`,
  `--save-baseline FILE`.
- Output format mirrors criterion (human-readable + JSON option).

## Definition of done

- [x] `riven bench` subcommand wired in `cli.rs`. *Shipped as
      `rivenc bench <file>` with `--filter` + `--iter-hint`.*
- [x] At least 5 benches in `crates/rivenc/benches/` for compile
      pipeline (parse, resolve, typeck, lower, codegen). *Shipped
      in `src/rivenc/benches/{parse,resolve,typeck,mir,codegen}_bench.rs`.*
- [ ] CI runs benches on PRs (warn-only, not blocking). *No CI
      workflows in tree; wiring deferred to whenever GH Actions
      lands.*
- [x] CHANGELOG bullet.
