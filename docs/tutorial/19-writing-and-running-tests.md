# Writing and Running Tests

Ruxen ships a pure-Ruxen test framework. Tests live in `tests/**.rx`
inside your project. Each `.rx` file under `tests/` is a test file —
no method-name convention, no test attribute. Run them with
`ruxen test`.

## A first test

Example [implemented]:

```ruby
Tester.describe("Calculator") do |t: &var Tester|
  t.it("adds two numbers") do
    t.expect(1 + 2).to_eq(3)
  end
end
```

Put this in `tests/calculator.rx` and run:

```text
$ ruxen test
test result: ok. 1 passed; 0 failed; 0 pending
```

`ruxen new <name>` scaffolds a working `tests/example.rx` for you, so
the first `ruxen test` in a new project is already green.

## Structure

- `Tester.describe(name) do |t: &var Tester| ... end` [implemented] —
  opens a group. Always the first call in a test file. The explicit
  `|t: &var Tester|` block parameter type is required because block
  parameter inference does not yet carry the inner `T` to
  `t.expect(...)` call sites without it.
- `t.context(name) do |t: &var Tester| ... end` [implemented] — nested
  group. Inherits the parent group's `before` hooks (after-hook
  inheritance is [planned](../../TODO.md#t-test-framework-next)).
- `t.it(name) do ... end` [implemented] — one test case. Runs in a
  forked child process, so an abort inside the body cannot poison
  sibling tests.
- `t.xit(name) do ... end` [implemented] — pending test. The body is
  not executed; the case counts toward `pending` in the summary.
- `t.before do ... end` [implemented] — runs before every `it` in this
  group.
- `t.after do ... end` [implemented] — runs after every `it` in this
  group.

## Matchers

- `t.expect(actual).to_eq(expected)` [implemented] — equality (the
  type needs an `==`). Marks the case as failed if the comparison fails.
- `t.expect(actual).not_to_eq(expected)` [implemented] — inequality.
- `BoolMatcher.new(actual).to_be_truthy` / `.to_be_falsy`
  [implemented] — boolean assertions that need no `==` on the type.
- `OptionMatcher.new(option.present?).to_be_nil` / `.not_to_be_nil`
  [implemented].
- `ArrayMatcher.new(&xs).to_include(value)` [implemented].
- `StringMatcher.new(&s).to_include(needle)` [implemented].

The unified `t.expect(...)` returns a `Matcher[T]` and only reaches
the equality matchers today. For the boolean / option / array /
string matchers, instantiate the matcher class directly (see above).
All four matcher classes stay, and reaching them through
`t.expect(...)` is [planned](../../TODO.md#t-d12).

## Expecting an abort

Use `t.it_panics(name, expected_substring) do ... end` instead of
`t.it(...)`. Example [implemented]:

```ruby
t.it_panics("explodes on overflow", "overflow") do
  abort("overflow")  # or any code that ends the process abnormally
end
```

The test passes if the body causes the case to exit abnormally (abort,
a runtime panic, a signal). Today the substring is recorded but not
verified — any abnormal exit from the body satisfies the assertion.
Substring verification is [planned](../../TODO.md#t-test-framework-next),
once parent-side stderr capture lands.

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
- An abort in one test does not stop sibling tests.
- Tests can mutate process-level state (env vars, the current-runner
  slot, FFI handles) without leaking changes into other tests.
- The OS reclaims any allocations the test leaks.

Per-file parallelism is bounded by `--test-threads` (default
`min(ncpus, 8)`); compilation runs serially to keep the incremental
cache's `manifest.bin` consistent, then the produced binaries fan out
across worker threads.

## Command-line options

- `ruxen test FILTER` [implemented] — substring filter on the test
  file path (e.g. `ruxen test calculator` runs only
  `tests/calculator*.rx`).
- `ruxen test --release` [implemented] — build tests with `--release`
  optimisation.
- `ruxen test --test-threads=N` [implemented] — limit parallelism
  (default `min(ncpus, 8)`). Set to `1` for fully serial execution.
- `ruxen test --fail-fast` [implemented] — stop dispatching after
  first failure.
- `ruxen test --nocapture` [implemented] — print captured
  stdout/stderr live, even for passing tests.
- `ruxen test --list` [implemented] — list discovered tests; don't
  compile or run.
- `ruxen test --no-run` [implemented] — build all test binaries; don't
  execute.
- `ruxen test --include-pending` — execute `xit` blocks too. The flag
  is accepted; running the bodies is
  [planned](../../TODO.md#t-test-framework-next).
- `ruxen test --format=pretty|tap|json` [implemented] — output format.

## Not available yet

These behaviours are [planned](../../TODO.md#t-test-framework-next):

- `let` / `subject` memoised helpers [planned](../../TODO.md#t-test-framework-next).
- `before(:all)` / `after(:all)` / `around` hooks [planned](../../TODO.md#t-test-framework-next).
- Custom matchers, `change { }`, `to_satisfy`, `to_be_a(ClassName)` [planned](../../TODO.md#t-test-framework-next).
- Mocking / `double` / partial stubbing [planned](../../TODO.md#t-test-framework-next).
- Shared example groups [planned](../../TODO.md#t-test-framework-next).
- Property testing [planned](../../TODO.md#t-test-framework-next).
- `--timeout` (per-test wall-clock cap) [planned](../../TODO.md#t-test-framework-next).
- Parent-side substring verification for `it_panics` [planned](../../TODO.md#t-test-framework-next).
- After-hook inheritance from `context` to nested children [planned](../../TODO.md#t-test-framework-next).
