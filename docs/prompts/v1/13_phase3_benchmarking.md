# 13 — Phase 3: benchmarking (T3.05)

**Depends on:** prompt 11.
**Reads:** `docs/requirements/tier3_05_benchmarking.md`.

## Goal

`riven bench` runs `@[bench]` annotated functions in a project and
reports per-iteration timing.

## Surface

```riven
@[bench]
def bench_string_concat(b: &mut Bencher)
  b.iter || -> {
    let mut s = String.new
    for i in 0..100
      s.push_str("xx")
    end
    s
  }
end
```

## TDD

- Unit test: `Bencher::iter(closure)` runs the closure ≥ N times,
  reports min/median/p99 in ns.
- Integration test: a fixture project with two `@[bench]` fns runs
  via `riven bench`, parses output, asserts timings reported.

## Implementation

- Add `crates/riven-bench/` (or fold into `riven-cli`).
- `Bencher` runtime tracks iterations, uses `clock_gettime`
  monotonic clock.
- Auto-tunes iteration count for ≥100ms total run (criterion-style).
- CLI flags: `--filter <pat>`, `--iterations N`, `--baseline FILE`,
  `--save-baseline FILE`.
- Output format mirrors criterion (human-readable + JSON option).

## Definition of done

- [ ] `riven bench` subcommand wired in `cli.rs`.
- [ ] At least 5 benches in `crates/rivenc/benches/` for compile
      pipeline (parse, resolve, typeck, lower, codegen).
- [ ] CI runs benches on PRs (warn-only, not blocking).
- [ ] CHANGELOG bullet.
