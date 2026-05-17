# 19 — Phase 5: test framework (T3.03)

**Depends on:** Phase 2 stdlib (need assertions + Option/Result).
**Reads:** `docs/requirements/tier3_03_test_framework.md`.

## Goal

`riven test` runs functions marked with a `test` directive in a
project, reports pass/fail/ignore counts, captures panics.

## Surface

```riven
def adds_two_numbers
  test
  assert_eq!(2 + 2, 4)
end

def slow_test
  test
  ignore
  ...
end

def bad_input
  test
  should_panic "expected message"
  panic!("expected message")
end
```

`test`, `ignore`, and `should_panic` are in-body directives — the
same shape as `include`, `inline :name`, `deprecated`,
`bench`. The directive lives at the top of the function body and
marks the function for the test runner.

`std.test` module:
- `assert!(cond)`, `assert_eq!(a, b)`, `assert_ne!(a, b)` — compiler-
  aware `!` forms (per spec §3.13).
- `panic!(msg)` — already exists.

## TDD

- Unit test: a project with 3 tests (1 pass, 1 fail, 1 ignored)
  runs via `riven test` and prints expected report.
- Integration: `--filter <pat>` skips non-matching.
- Integration: `--exact` matches name exactly.
- E2E: parallel test execution (multiple OS threads, each running
  one `test`-marked function).

## Implementation

- `riven test` subcommand in CLI.
- Per `test`-marked function, emit a thunk that calls into a
  registry; the registry binary collects all thunks at link time.
- Test runner spawns each test on its own thread with a panic-catch
  hook.
- Output format: same as cargo test.

## Definition of done

- [ ] `riven test` works on a fixture project.
- [ ] Filter, exact, ignored, should_panic all behave correctly.
- [ ] Parallel execution across cores.
- [ ] CHANGELOG bullet.
