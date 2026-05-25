# 19 — Phase 5: test framework (T3.03)

> **Decisions locked 2026-05-22** in the CLI-consolidation session.
> Implementation is deferred to a later session — this doc captures the
> surface so the eventual implementer doesn't relitigate it.

**Depends on:** Phase 2 stdlib (need assertions + Option/Result).
**Reads:** `docs/requirements/tier3_03_test_framework.md`.

## Goal

`ruxen test` runs functions matching `def test_*(t: &var Tester)` in a
project, reports pass/fail counts, captures panics.

## Locked decisions

| Aspect | Choice |
|---|---|
| CLI entry | `ruxen test` (subcommand on the unified `ruxen` binary). The `Test` variant already exists in `src/ruxen_cli/src/cli.rs` and exits 2 with a pointer to this doc until shipped. |
| Test dir | `test/` — any `.rx` file under it. Repo-root + per-package. |
| Test fn shape | `def test_*(t: &var Tester)` — name-prefix convention, matches `bench_*` discovery so we reuse the bench machinery. |
| Helper class | `Tester` — parallel to `Bencher`. Lives in `library/std/test/src/lib.rx`. |
| Assertions | `t.expect(value).to_eq(other)` builder pattern. Surface: `to_eq`, `to_ne`, `to_be_true`, `to_be_false`, `to_contain`, `to_throw`. |
| Optional DSL | `describe(ctx, "name", do |ctx| ... end)` + `it` layered on top of `Tester` once the base layer is solid — not v1 requirement. |
| Discovery | Walk `test/**/*.rx` from cwd; collect `def test_*` fns; synth a `def main` that runs each with a fresh `Tester`; aggregate pass/fail. Mirrors `ruxenc bench`'s synth-main approach exactly. |
| Failure semantics | Test fn keeps running after assert failure (record-and-continue). Opt-in `t.fail_fast()` aborts the current fn on first failure. |
| Filter | `ruxen test --filter <pat>` — substring match on test fn name. Same shape as bench. |

## Where things live (precedent)

The pure-Ruxen `Bencher` pattern is the precedent. Mirror it:

- `library/std/bench/src/lib.rx` → `library/std/test/src/lib.rx`
- `src/ruxenc/src/bench.rs` (lib fn `bench::run`) → `src/ruxenc/src/test.rs` (lib fn `test::run`)
- `ruxen_cli::Command::Bench` → `ruxen_cli::Command::Test`
- Synth-main approach: parse file, collect `test_*` fns, append a `def main` that constructs a `Tester` and calls each. Compile + run through the normal pipeline.

No compiler-side parser/keyword work. No directive bodies. No `#[test]`-equivalent attribute. Discovery is pure name convention — `test_*` — exactly the rule `bench_*` follows today.

## Future surface (NOT v1)

The original draft included in-body directives (`test`, `ignore`, `should_panic "expected message"`) and bang-form assertions (`assert!(cond)`, `assert_eq!(a, b)`). These are explicitly deferred:

- `ignore` → use `--filter` exclusion or a `t.skip("reason")` call inside the test body.
- `should_panic` → `t.expect_throw(do || ... end)` builder method (the `to_throw` matcher above covers this for expression-level panics).
- `assert!` macros → not v1; the `Tester` builder methods cover the same surface without the macro layer.

If/when Ruxen gains a macro system, the bang-form assertions can be added as sugar over the builder methods without churning the runtime.

## TDD plan (when the implementer picks this up)

- Unit test: a project with 3 tests (1 pass, 1 fail) runs via `ruxen test` and prints expected report.
- Integration: `--filter <pat>` skips non-matching.
- E2E: parallel test execution (multiple OS threads, each running one `test_*`-marked function) — gated on whether the v1 runtime exposes a thread-spawn API by then; serial-only is acceptable for first cut.

## Implementation sketch (locked)

1. Add `library/std/test/` package with `Tester` class + `Expect` builder.
2. Add `src/ruxenc/src/test.rs` with `pub fn run(args: &[String]) -> Result<(), String>`.
3. Replace the `cli::Command::Test` stub in `src/ruxen_cli/src/main.rs` with a real dispatch into `ruxenc::test::run`.
4. Output format: same shape as `cargo test` — `running N tests`, per-test `ok`/`FAILED` lines, summary footer.

## Definition of done

- [ ] `library/std/test/src/lib.rx` defines `Tester` + `Expect`.
- [ ] `ruxen test` works on a fixture project (3 tests, mixed pass/fail).
- [ ] `--filter` behaves correctly.
- [ ] CHANGELOG bullet.
