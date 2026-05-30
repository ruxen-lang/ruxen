# Writing and Running Tests

Ruxen ships a pure-Ruxen test framework. Tests live in `tests/**.rx`
inside your project. Each `.rx` file under `tests/` is a test file —
no method-name convention, no `#[test]` attribute. Run them with
`ruxen test`.

## A first test

```rx
Tester.describe("Calculator") do |t: &var Tester|
  t.it("adds two numbers") do
    t.expect(1 + 2).to_eq(3)
  end
end
```

Put this in `tests/calculator.rx` and run:

```
$ ruxen test
test result: ok. 1 passed; 0 failed; 0 pending
```

`ruxen new <name>` scaffolds a working `tests/example.rx` for you, so
the first `ruxen test` in a new project is already green.

## Structure

- `Tester.describe(name) do |t: &var Tester| ... end` — opens a group.
  Always the first call in a test file. The explicit
  `|t: &var Tester|` binding type is required in v1 because closure
  parameter inference does not yet propagate the inner `T` at
  `t.expect(...)` call sites without it.
- `t.context(name) do |t: &var Tester| ... end` — nested group.
  Inherits the parent group's `before` hooks (after-hook inheritance
  ships in v1.1).
- `t.it(name) do ... end` — one test case. Runs in a forked child
  process, so a panic inside the body cannot poison sibling tests.
- `t.xit(name) do ... end` — pending test. The body is not executed;
  the case counts toward `pending` in the summary.
- `t.before do ... end` — runs before every `it` in this group.
- `t.after do ... end` — runs after every `it` in this group.

## Matchers

- `t.expect(actual).to_eq(expected)` — equality (requires
  `T: PartialEq`). Marks the case as failed if the comparison fails.
- `t.expect(actual).not_to_eq(expected)` — inequality.
- `BoolMatcher.new(actual).to_be_truthy` / `.to_be_falsy` — boolean
  assertions decoupled from the `PartialEq` constraint.
- `OptionMatcher.new(option.is_some).to_be_nil` / `.not_to_be_nil`.
- `ArrayMatcher.new(&xs).to_include(value)`.
- `StringMatcher.new(&s).to_include(needle)`.

The unified `t.expect(...)` returns a `Matcher[T]` and only dispatches
to the equality matchers in v1. For the boolean / option / array /
string matchers, instantiate the matcher class directly (see above).

## Expecting a panic

Use `t.it_panics(name, expected_substring) do ... end` instead of
`t.it(...)`:

```rx
t.it_panics("explodes on overflow", "overflow") do
  Runner.panic  # or any code that ultimately calls ruxen_panic
end
```

The test passes if the body causes the case to exit abnormally (panic,
abort, signal). In v1 the substring is recorded but not verified —
any panic from the body satisfies the assertion. Substring verification
ships in v1.1 once parent-side stderr capture lands.

## Hooks and helpers

`before` and `after` run before / after each test in the surrounding
group. Nested `context` blocks inherit the surrounding group's
`before` hooks (outer-first ordering — outer `before` runs, then
inner `before`, then the body).

Shared helpers (factories, fixtures, custom assertions) go in
`tests/support/**.rx` — those files are NOT executed as test files.
The discovery walker explicitly skips the `tests/support/` subtree.

## Process isolation

Every `t.it` runs in a forked child process. This means:
- A panic in one test does not abort sibling tests.
- Tests can mutate process-level state (env vars, the current-runner
  slot, FFI handles) without leaking changes into other tests.
- The OS reclaims any allocations the test leaks — useful while
  `Drop` semantics are still landing in v1.

Per-file parallelism is bounded by `--test-threads` (default
`min(ncpus, 8)`); compilation runs serially to keep the incremental
cache's `manifest.bin` consistent, then the produced binaries fan out
across worker threads.

## Command-line options

- `ruxen test FILTER` — substring filter on the test file path
  (e.g. `ruxen test calculator` runs only `tests/calculator*.rx`).
- `ruxen test --release` — build tests with `--release` optimisation.
- `ruxen test --test-threads=N` — limit parallelism (default
  `min(ncpus, 8)`). Set to `1` for fully serial execution.
- `ruxen test --fail-fast` — stop dispatching after first failure.
- `ruxen test --nocapture` — print captured stdout/stderr live, even
  for passing tests.
- `ruxen test --list` — list discovered tests; don't compile or run.
- `ruxen test --no-run` — build all test binaries; don't execute.
- `ruxen test --include-pending` — execute `xit` blocks too (v1.1).
- `ruxen test --format=pretty|tap|json` — output format.

## What is not in v1

These behaviours are documented as planned for v1.1 — track in the
test framework spec at
`docs/superpowers/specs/2026-05-23-test-framework-design.md`:

- `let` / `subject` memoised helpers.
- `before(:all)` / `after(:all)` / `around` hooks.
- Custom matchers, `change { }`, `to_satisfy`, `to_be_a(ClassName)`.
- Mocking / `double` / partial stubbing.
- Shared example groups.
- Property testing.
- `--timeout` (per-test wall-clock cap).
- Parent-side substring verification for `it_panics`.
- After-hook inheritance from `context` to nested children.
