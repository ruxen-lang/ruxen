# 19 — Phase 5: test framework (T3.03)

**Depends on:** Phase 2 stdlib (need assertions + Option/Result).
**Reads:** `docs/requirements/tier3_03_test_framework.md`.

## Goal

`riven test` runs `@[test]` functions in a project, reports
pass/fail/ignore counts, captures panics.

## Surface

```riven
@[test]
def adds_two_numbers
  assert_eq!(2 + 2, 4)
end

@[test]
@[ignore]
def slow_test
  ...
end

@[test]
@[should_panic("expected message")]
def bad_input
  panic "expected message"
end
```

`std::test` module:
- `assert!(cond)`, `assert_eq!(a, b)`, `assert_ne!(a, b)` — macros or
  fns; whichever fits the language.
- `panic!(msg)` — already exists.

## TDD

- Unit test: a project with 3 tests (1 pass, 1 fail, 1 ignored)
  runs via `riven test` and prints expected report.
- Integration: `--filter <pat>` skips non-matching.
- Integration: `--exact` matches name exactly.
- E2E: parallel test execution (multiple OS threads, each running
  one `@[test]`).

## Implementation

- `riven test` subcommand in CLI.
- Per `@[test]` fn, emit a thunk that calls into a registry; the
  registry binary collects all thunks at link time.
- Test runner spawns each test on its own thread with a panic-catch
  hook.
- Output format: same as cargo test.

## Definition of done

- [ ] `riven test` works on a fixture project.
- [ ] Filter, exact, ignored, should_panic all behave correctly.
- [ ] Parallel execution across cores.
- [ ] CHANGELOG bullet.
