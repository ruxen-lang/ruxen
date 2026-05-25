# Test Framework — Design

Date: 2026-05-23
Status: draft (awaiting user review before plan)
Supersedes (for v1): `docs/requirements/tier3_03_test_framework.md` — that spec specified parser/AST/HIR changes; this design ships entirely as a pure-`.rx` package + CLI subcommand, mirroring the `library/std/bench/` precedent and `feedback_pure_ruxen_first.md`.

---

## 1. Summary

Add a pure-Ruxen test framework. A single `library/std/test/` package supplies an RSpec-flavored DSL keyed on a `Tester` instance yielded into block arguments. A new `ruxen test` CLI subcommand discovers every `.rx` file under `tests/**`, wraps each in a synthesized `def main`, builds one binary per file through the existing incremental cache, and executes them in parallel with fork-per-test isolation. No parser, AST, HIR, MIR, or codegen change is required.

---

## 2. Goals & non-goals

### Goals

1. Write a test in Ruxen inside `tests/**.rx` using a familiar RSpec-shaped DSL.
2. No method-name convention — the file is the discovery unit. Any `.rx` under `tests/` is a test file.
3. No compiler changes. Everything lives in `library/std/test/` + `crates/ruxen-cli/src/test.rs`.
4. A panic in one test does not abort the run; other tests continue.
5. Incremental: a second `ruxen test` with no source changes does no recompilation and only re-executes the requested tests.
6. Parallel test execution by default (`min(ncpus, 8)`); `--test-threads=1` opts out.
7. Lean default output: progress dots while running, full detail only for failures.
8. Stable machine-readable outputs (`--format=tap`, `--format=json`) for CI.

### Non-goals (v1)

- `let` / `subject` (memoized lazy helpers — requires RSpec-style instance_eval, deferred).
- `change { }`, custom matchers, `to_satisfy(&block)`.
- Mocking / `double` / `instance_double`.
- Shared examples (`shared_examples`, `it_behaves_like`).
- `before(:all)` / `after(:all)` / `around` hooks (only `before` / `after` per-example in v1).
- Property testing (separate tier-3 item).
- Snapshot / golden testing.
- Coverage.
- Test timeouts (`--timeout`) — v2, needs threading discipline.
- Windows (no `fork()`); v1 is Linux + macOS.

---

## 3. Surface

### 3.1 Test file shape

Every `.rx` file under `tests/**` is a test file. No method-name convention. The user writes only `Tester` DSL calls — no `def`, no `main`:

```rx
use std.test.Tester

Tester.describe("Calculator") do |t|
  t.before do
    # runs before each `it` in this group (and its nested contexts)
  end

  t.context("addition") do |t|
    t.it("adds two numbers") do
      t.expect(Calculator.add(1, 2)).to_eq(3)
    end

    t.it("is commutative") do
      t.expect(Calculator.add(2, 3)).to_eq(Calculator.add(3, 2))
    end
  end

  t.context("errors") do |t|
    t.it("panics on overflow") do
      t.expect_panic("overflow") do
        Calculator.add(Int.max, 1)
      end
    end

    t.xit("not yet implemented") do
    end
  end
end
```

### 3.2 API

`Tester` is a class in `library/std/test/src/lib.rx`. Only `Tester.describe` is a class method (the entry point); all nested calls are instance methods on the `t` yielded into each block. Nested `context` yields a fresh child `Tester` linked to its parent.

| Call | Where | Effect |
|---|---|---|
| `Tester.describe(name) do \|t\| ... end` | top of test file | Constructs a root group, yields, attaches result to the active `Runner`. |
| `t.context(name) do \|t\| ... end` | inside a `describe` / `context` | Constructs a child group, inherits parent hooks. |
| `t.it(name) do ... end` | inside a group | Registers one test case in the current group. |
| `t.xit(name) do ... end` | inside a group | Registers a pending test (block is not executed). |
| `t.before do ... end` | inside a group | Runs before each `it` in this group and descendants. |
| `t.after do ... end` | inside a group | Runs after each `it` in this group and descendants (even on failure). |
| `t.expect(x) -> Matcher[T]` | inside an `it` | Wraps `x` for assertion. |
| `t.expect_panic(substring) do ... end` | inside an `it` | Asserts the block panics with a message containing `substring`. |

### 3.3 Matchers (v1)

`Matcher[T]` is the return of `expect(x)`. Methods:

- `to_eq(expected)` / `not_to_eq(expected)` — equality. Requires `T: PartialEq`. Failure renders both sides via `Displayable`; non-`Displayable` types render `<opaque>` (acceptable v1 degradation, same constraint the tier3_03 spec already accepted).
- `to_be_truthy` / `to_be_falsy` — boolean / nullable checks.
- `to_include(item)` — for `Array[T]` and `String`. Element-equality via `PartialEq`.
- `to_be_nil` / `not_to_be_nil` — for `Option[T]` / nullable values.
- `to_be_a(ClassName)` — runtime class check.

`not_to_*` mirrors each positive matcher where it makes sense; v1 explicitly ships `not_to_eq`, `not_to_be_nil`. Other `not_to_*` deferred.

### 3.4 Discovery & naming

- Test files: every `.rx` under `tests/**` (relative to the package root).
- Test path = relative file path with `/` → `.` and `.rx` dropped.
  - `tests/calculator/math.rx` → `calculator.math`
- Full test case name = `<test-path> › <describe> › <context...> › <it>`. The `›` separator is used in pretty output; for filters use `/` (regex-friendly): `ruxen test calculator/math/addition/adds`.

### 3.5 CLI

```
ruxen test                                  # build + run all tests
ruxen test FILTER                           # substring filter on test name (positional)
ruxen test --release                        # build tests in release mode
ruxen test --test-threads=N                 # parallel fan-out width (default: min(ncpus, 8))
ruxen test --fail-fast                      # stop dispatching new tests after first failure
ruxen test --nocapture                      # don't capture test stdout/stderr; pass through live
ruxen test --list                           # list discovered tests; don't run
ruxen test --no-run                         # build but don't execute
ruxen test --include-pending                # also execute `xit` blocks
ruxen test --format=pretty|tap|json         # output format (default: pretty)
```

Semantics match `cargo test` where overlapping. Positional FILTER is substring on the full test name (path + describe + contexts + it).

### 3.6 Output

Default (`--format=pretty`):

```
running 7 tests (3 files)
calculator.math: ....F.x
arithmetic.basic: ..

FAILED: calculator.math › Calculator › addition › adds two numbers
  expected: 3
  actual:   4
  at tests/calculator/math.rx:7

test result: FAILED. 5 passed; 1 failed; 1 pending; finished in 0.08s
```

TAP (`--format=tap`):

```
TAP version 13
1..7
ok 1 - calculator.math › Calculator › addition › is commutative
not ok 2 - calculator.math › Calculator › addition › adds two numbers
  ---
  message: "expected 3, got 4"
  at: tests/calculator/math.rx:7
  ...
ok 3 - calculator.math › Calculator › errors › not yet implemented # SKIP pending
...
```

JSON (`--format=json`): one event per line; schema-compatible with `cargo test --format=json` where practical.

---

## 4. Architecture

### 4.1 `std.test` package

New directory `library/std/test/`:

```
library/std/test/
  Ruxen.toml
  src/
    lib.rx            # public entry: Tester, Matcher, Runner, free imports
    runner.rx         # Runner — owns the group tree, executes test cases
    matcher.rx        # Matcher[T] implementations
  # no runtime/ directory in v1 — fork/wait via std.process / std.sync,
  # panic via existing ruxen_panic
```

#### `class Runner`
- Holds the group tree being built by the current file's `describe` calls.
- Stored in a thread-local "active runner" slot. `Tester.describe` reads this slot to attach a new root group; if absent, `describe` is a no-op (so spec files can be parsed standalone without crashing the compiler — defensive).
- `Runner.new(test_path)` opens the slot. `Runner.execute()` walks the tree and dispatches.

#### `class Tester`
- Represents one group (a `describe` or `context`).
- Fields: `name: String`, `parent: Tester?`, `hooks_before: Array[Fn() -> ()]`, `hooks_after: Array[Fn() -> ()]`, `children: Array[Tester]`, `cases: Array[TestCase]`.
- Methods: `describe` (class, root only), `context`, `it`, `xit`, `before`, `after`, `expect`, `expect_panic`.

#### `class TestCase`
- Fields: `name: String`, `body: Fn() -> ()`, `pending: Bool`, `expect_panic_substr: String?`.
- Owns the closure produced by `it { ... }`; the runner invokes it inside the post-fork child.

#### `class Matcher[T]`
- Fields: `actual: T`, `location: (file: String, line: Int)` — captured at `expect` call site via a `__caller_location` intrinsic *(see Open Q 1)*.
- Methods: matcher set per §3.3. Each matcher on failure calls `ruxen_panic` with a structured message the parent decodes.

#### Free re-imports
`library/std/test/src/lib.rx` re-exports `Tester` only. The DSL is exclusively instance methods on `t`; there are no free `it`/`expect`/`before`/etc. functions.

### 4.2 Compile-time wrap

`ruxen test`, before invoking `ruxenc`, synthesizes one `.rx` file per test file at `target/ruxen/test-build/<test-path>.synth.rx` by **textual concatenation** of three parts:

1. Prelude (auto-generated):
   ```rx
   # AUTO-GENERATED from tests/calculator/math.rx — do not edit.
   use std.test.Tester
   use std.test.Runner

   def main
     let r = Runner.new("calculator.math")
   ```
2. The user's test file body verbatim (line-for-line; see R1 for line-number handling).
3. Postlude:
   ```rx
     r.execute
   end
   ```

The synthesized file is what `ruxenc` compiles. No `#include`-style directive is added to the language — concatenation is purely a tooling step inside `ruxen test`.

### 4.3 Constraints on test file contents

Because the wrap is textual concatenation into a `def main` body, a `tests/**.rx` file:

- May contain only **expression statements** at top level (`Tester.describe(...) do ... end`, `let x = ...`, etc.) — anything legal inside a `def` body.
- May **not** contain top-level `def`, `class`, `mixin`, or `use` statements after the prelude — those would be syntactically illegal inside the synthesized `def main`.
- May not redefine `main` (the wrap already supplies it).

**Helper code** (factories, fixtures, shared `def`s) lives in `tests/support/**.rx`. The discovery walker excludes `tests/support/` from the test-file list; those files are compiled as regular modules and imported by name from test files via `use tests.support.factories` *(exact module path TBD in implementation — depends on how `tests/` is registered as a module root; see Open Q 2)*.

### 4.4 Build pipeline

For each discovered test file:

1. Compute wrapper path: `target/ruxen/test-build/<test-path>.wrapper.rx`.
2. Synthesize wrapper (Option A above).
3. Invoke `ruxenc` with `BuildOptions { flags: "test", output: target/debug/test/<test-path> }`.
4. The existing incremental cache (`ruxenc/src/cache/`) keys on source content hash + flags; rebuilds only changed wrappers.

Per-file binary (vs one monolithic binary):
- Touching one test file rebuilds one binary. Touching `library/std/test/` rebuilds all.
- Cleanly maps onto parallel execution (Phase C).
- Cleanly maps onto isolation: a corrupted test binary doesn't taint others.

### 4.5 Execution pipeline

`ruxen test` (the outer dispatcher) is a Rust process. For each test binary it dispatches:

```
   ruxen test (Rust)
     │ spawn min(ncpus, 8) at a time
     ├── target/debug/test/calculator/math   (Ruxen binary)
     │     │ on startup, reads argv (filter, format, --list, ...)
     │     │ for each registered TestCase:
     │     │   fork()
     │     │     child: run hook_before, run body, run hook_after, exit
     │     │     parent: waitpid, decode (pass/fail/panic/expect_panic match)
     │     │ emit per-case event to stdout (TAP-like internal protocol)
     │     └── exit
     ├── target/debug/test/arithmetic/basic
     └── ...
```

- **Fork-per-test** isolates panics, OOM, infinite recursion. No unwinding required. Side effect: each test starts from a clean heap, hiding leaks (acceptable until `Drop` lands per memory).
- **Inter-process protocol:** child writes one structured line to stdout per case start/finish in a stable format the parent parses and re-renders into the user-selected `--format`. Captured stdout/stderr of the test body itself is on a separate FD (the body's stdout is piped to a buffer, only printed on failure unless `--nocapture`).
- **Outer parallelism:** `ruxen test` dispatches up to `min(ncpus, 8)` test binaries concurrently. **Inner parallelism:** each binary forks up to N children concurrently (same cap). Total concurrency is bounded; `--test-threads=1` serializes both.

### 4.6 Panic catching

The existing `ruxen_panic` in `library/std/core/runtime/alloc.c` calls `abort()`. That's exactly what fork-per-test needs:
- Child aborts → parent sees SIGABRT exit → records `FAILED` with the captured stderr message.
- `expect_panic("substr")` asserts the child aborted AND captured stderr contains `substr`.

No new runtime entry point required. The test runtime reuses what's already there.

### 4.7 Cache integration

The existing `ruxenc/src/cache/driver.rs` already keys on `BuildOptions.flags`. Test builds pass `flags="test"`, keeping test object files in a separate cache namespace from `ruxen build`.

Additionally, `ruxen test` maintains a discovery cache at `target/ruxen/test-cache/discovery.json` keyed on (file path, mtime, content hash) so repeated runs over unchanged test directories skip the walk + parse phase entirely.

### 4.8 Files touched

| Kind | Path | Change |
|---|---|---|
| new pkg | `library/std/test/Ruxen.toml` | New package manifest |
| new pkg | `library/std/test/src/lib.rx` | `Tester` class + class entry `describe` |
| new pkg | `library/std/test/src/runner.rx` | `Runner` + `TestCase` |
| new pkg | `library/std/test/src/matcher.rx` | `Matcher[T]` + v1 matchers |
| new cli | `crates/ruxen-cli/src/test.rs` | Discovery, wrap, build, dispatch, output (~400 LOC) |
| new cli | `crates/ruxen-cli/src/test_output.rs` | Pretty / TAP / JSON renderers (~150 LOC) |
| edit cli | `crates/ruxen-cli/src/cli.rs` | Add `Command::Test { ... }` |
| edit cli | `crates/ruxen-cli/src/main.rs` | Wire `Command::Test` to `test::run` |
| edit cli | `crates/ruxen-cli/src/scaffold.rs` | `ruxen new` drops `tests/example.rx` |
| edit | `library/BOOTSTRAP_FILES` | Register `library/std/test/src/*.rx` per `project_ruxen_bootstrap_files_load_check.md` (orphan-load risk — required) |
| new doc | `docs/tutorial/17-testing.md` | Tutorial page |
| new tests | `crates/ruxen-cli/tests/test_runner.rs` | Integration tests on fixture projects under `crates/ruxen-cli/tests/fixtures/test-*` |

---

## 5. Test matrix (testing the test framework)

| Case | Expected |
|---|---|
| `t.it("x") { t.expect(1).to_eq(1) }` | ok |
| `t.it("x") { t.expect(1).to_eq(2) }` | FAILED with actual=1 expected=2 + file:line |
| `t.xit("x") { ... }` | pending; block not executed |
| `t.it("x") { panic!("boom") }` | FAILED, reported as panic, other tests continue |
| `t.it("x") { t.expect_panic("boom") { panic!("kaboom") } }` | ok (substring match) |
| `t.it("x") { t.expect_panic("boom") { } }` | FAILED — expected panic, none thrown |
| `t.before { ... }` in parent + `t.it` in child context | before runs before each `it`, including nested |
| `t.after { ... }` runs even when `it` fails | post-failure cleanup observed |
| 2 test files, 8 cores | Both build + run in parallel |
| `ruxen test foo` with no matches | Exit 0, prints "0 tests" |
| Repeated `ruxen test` with no source changes | Uses cache; no rebuild; only re-executes |
| `ruxen test --list --format=json` | One JSON event per discovered test, no execution |
| `ruxen test --nocapture` | Test's `puts` reaches stdout live |
| `ruxen test --fail-fast` | First failure stops outer dispatch |
| `ruxen test --test-threads=1` | All cases serialized |
| Test file with a syntax error | ruxenc emits diagnostic; that one binary marked build-failed; others still run |
| Test that hangs forever | (v2) `--timeout` kills it; v1 documents the constraint |

---

## 6. Phasing

| Phase | Scope | Est. days |
|---|---|---|
| 1 | `library/std/test/` package — `Tester`, `Runner`, `TestCase`, v1 matchers, no runner CLI yet (drives via hand-written wrapper to validate the DSL compiles and runs) | 1 |
| 2 | `crates/ruxen-cli/src/test.rs` — discovery, wrap, single-file build & run, pretty output | 1 |
| 3 | Fork-per-test isolation, parallel dispatch, cache integration, TAP + JSON formats | 0.5 |
| 4 | `ruxen new` scaffold, tutorial doc 17, integration test fixtures | 0.5 |

Total: **~3 engineer-days** (vs the original 9-day estimate). The compression comes entirely from skipping every compiler change.

---

## 7. Open questions

1. **`__caller_location` for matcher file:line.** Ruxen has no `caller_location` intrinsic today. Options:
   - (a) Add one — minor compiler addition, breaks the "no compiler change" thesis.
   - (b) Pass `(file, line)` explicitly into `expect(x, __FILE__, __LINE__)` — clunky.
   - (c) v1 omits file:line for matcher failures; show only test name + describe path. Failure message still includes the actual/expected diff.
   - **Recommend (c) for v1.** Add (a) later as a small follow-up.

2. **Helper files under `tests/`.** Users will want shared helpers (factories, fixtures). Three options:
   - (a) `tests/support/**.rx` is excluded from discovery and imported by name from test files.
   - (b) Files whose body doesn't contain `Tester.describe` are skipped (heuristic).
   - (c) Forbid helpers; helpers live in `src/test_helpers/` and are imported.
   - **Recommend (a)** — simple, matches RSpec's `spec/support/` convention.

3. **`expect_panic` substring matching.** Substring (default), regex, or exact? **Recommend substring** — matches Rust's `#[should_panic(expected = "…")]`.

4. **Default `--test-threads` cap.** Rust uses ncpus; we cap at 8 to avoid oversubscribing with the double fork (outer dispatch × inner fork). **Recommend `min(ncpus, 8)`**; revisit after measuring.

5. **Reporting leaked memory.** Until `Drop` ships for `String` / `Array`, every test leaks. Fork-per-test hides this (child exit reclaims). **Recommend documenting loudly in `docs/tutorial/17-testing.md`** rather than instrumenting.

6. **Top-level `def`s in test files.** Per §4.3 Option A, test files are DSL-only — they cannot contain top-level `def` or `use` past the first line. Acceptable v1 constraint. If a user needs helpers, put them in `tests/support/` (Open Q 2).

---

## 8. Risks

- **R1 — Concatenation-based include is fragile.** Line numbers in diagnostics may not match the user's source file. Mitigation: the synthesized wrapper file starts with a `# line N "tests/calculator/math.rx"` marker *if* ruxenc supports it; otherwise diagnostics show the wrapper path and the user manually maps lines. Investigate during Phase 2.
- **R2 — Thread-local active-runner slot.** `std.test` needs a single-slot per-process mutable cell to thread the active `Runner` from `Tester.describe` into the closure-time DSL. `std.sync` has the primitives; verify the bootstrap order works (per memory `project_ruxen_bootstrap_files_load_check.md` — moved-to-`.rx` declarations need a `BOOTSTRAP_FILES` entry).
- **R3 — Closure leak across fork.** The test body closure may capture parent-process state (e.g., a file handle opened in `before`). Document: per-example state must be set up inside the `before` block; cross-test state is not supported in v1 (no `before(:all)`).

---

## 9. Out-of-scope follow-ups (post-v1)

- `let` / `subject` memoized helpers.
- `before(:all)` / `after(:all)`.
- Custom matchers + `change { }` + `to_satisfy(&block)`.
- Mocking / `double`.
- Shared examples.
- Property testing.
- Snapshot testing.
- Coverage instrumentation.
- `--timeout`.
- Macro-based assertions with expression stringification (depends on tier-1 doc 05 macros).
- Windows support.
