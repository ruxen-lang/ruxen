# Tier 3.03 — Test Framework (`test` in-body directive + `riven test`)

Status: draft
Depends (soft): Tier-1 doc 05 (macros) for assertion macros; Tier-1 doc 01 (stdlib) for `fmt::Debug`; Tier-1 doc 04 (Drop) for non-leaking tests
Depends (hard): compiler-builtin `test` in-body directive recognition is the only hard requirement
Blocks: doc 05 (bench) shares 90% of the harness

---

## 1. Summary & motivation

Riven has no way to write tests in Riven. Everything under
`crates/riven-core/tests/` is Rust `#[test]` — that tests the *compiler*,
not programs written in Riven. The tutorial never discusses testing
(`docs/tutorial/` has no `testing.md`). A user building a Riven project
with `riven new foo` gets a `src/main.rvn` — no `tests/` directory,
no harness, no assertion functions.

This doc specifies the v1 test surface: the `test` in-body directive,
the `riven test` subcommand, an assertion API, test discovery, output
format, and the path from v1 (compiler-builtin) to v2
(macro-system-aware).

---

## 2. Current state

### 2.1 No `test` in-body directive

The prefix-attribute `@[...]` surface is retired entirely (replaced by
`lib` options for link names and body-level `layout` for repr). No
in-body `test` keyword is recognized by the parser. A `test` directive
on its own line inside a `def` body is currently treated as a regular
identifier reference and fails resolution.

### 2.2 No `riven test` subcommand

`riven-cli`'s `Command` enum (`crates/riven-cli/src/cli.rs:25-114`) has
no `Test`, `Bench`, or `Doc` variant. Only `New`, `Init`, `Build`, `Run`,
`Check`, `Clean`, `Add`, `Remove`, `Update`, `Tree`, `Verify`.

### 2.3 No test binary / runner

There is no code anywhere in the workspace that collects functions
tagged with an attribute and emits them into a test binary. The MIR
lowerer (`crates/riven-core/src/mir/lower.rs`) walks `HirProgram.items`
and lowers every function present, emitting `main` as the entry point.
Test entry-point generation requires a new code path.

### 2.4 No assertion API

No `assert!`, `assert_eq!`, `assert_ne!` macros or functions. The only
panic-like primitive is `panic!` routed through `runtime/runtime.c:423-426`
(`riven_panic(const char*)` → `abort()`). Assertion messages would land
here.

### 2.5 Existing conventions that inform shape

- Body-level directives are bare keywords on their own line inside a
  type or function body (Ruby-style — same shape as `private`,
  `include`, `layout`).
- Doc comments are `##` (§doc 04).
- Function definitions are `def name(args) -> Ret ... end`.
- `riven-cli` uses clap; `Build` takes `--release` and `--bin`.
- The cache (`rivenc/src/cache/driver.rs`) produces per-file
  `CompileOutput` — reusable for test-build caching.

---

## 3. Goals & non-goals

### Goals

1. Write a test in Riven: put a `test` directive inside a `def` body
   and `riven test` runs it.
2. Tests in `tests/*.rvn` (integration-style) and inline in `src/**.rvn`
   (unit-style).
3. An assertion API that prints the failing expression, the expected
   value, the actual value, and a file:line.
4. A runner that reports pass / fail / skip, ignores panics (reports
   them as failures), and exits non-zero if any test failed.
5. Filter by name substring: `riven test foo.bar`, `riven test
   --filter bar`.
6. Parallel execution of independent tests by default; `--test-threads=1`
   overrides.
7. Stable output format that works as both human-readable and CI-parseable
   (TAP-compatible subset or Rust-style).
8. No external deps — ships as part of the toolchain.

### Non-goals

- **Property testing** — see doc 08.
- **Fixtures / setup / teardown hooks.** `before_each` directive etc. are v2.
- **Golden / snapshot testing.** v2.
- **Mocking framework.** v2.
- **Benchmarks.** See doc 05 (sibling).
- **Async tests.** Blocked on tier-1 doc 03 (async).
- **Test doubles.** v2.
- **Coverage.** Coverage via `cargo-tarpaulin` / `llvm-cov` is a
  post-v1 item; the infrastructure (DWARF, debug info, doc 02) needs
  to land first.

---

## 4. Surface

### 4.1 In-body directives

Primary test directive:

```
def adds_two_numbers
  test
  assert_eq(2 + 2, 4)
end
```

Extended directives (v1, written on their own lines at the top of the fn body):

```
def x
  test
  ignore                       # mark test as skipped (still discovered)
  # OR with reason:
  ignore "reason"              # skip with reason shown in output
  ...
end

def y
  test
  only                         # "focus" — if any test has `only`, only those run
  ...
end

def z
  test
  should_panic                 # test passes iff it panics
  # OR with substring matcher:
  should_panic "substring"
  ...
end
```

v2 extensions (post-macros doc 05):

```
def x
  test
  timeout 1000                 # ms; v2, needs threading
  threads 1                    # force serial for this one test
  ...
end
```

### 4.2 CLI

```
riven test                                  # build + run all tests
riven test --release                        # release-mode tests
riven test foo.bar                          # filter by substring (run tests whose path contains "foo.bar")
riven test --filter="sub"                   # explicit filter form
riven test --test-threads=1                 # serialize
riven test --nocapture                      # show stdout/stderr of tests (default: capture)
riven test --no-run                         # build but don't execute (useful for debugging)
riven test --ignored                        # run only tests with `ignore` directive
riven test --include-ignored                # run both ignored and normal
riven test --exact                          # filter matches must be exact, not substring
riven test --list                           # list discovered tests, don't run
riven test --format=pretty|tap|json         # output format, default pretty
riven test --bin main                       # limit to tests in a specific binary
```

### 4.3 Assertion API

Ship as compiler-builtin function calls in v1; migrate to macros (tier-1
doc 05) in v2 so that stringification of the expression works.

v1 functions (in `std.test` module once stdlib ships; before that
as builtins registered by `resolve::mod::register_builtins`):

```
def assert(cond: Bool)
def assert_msg(cond: Bool, msg: &str)
def assert_eq[T](actual: T, expected: T)  where T: Debug + Eq
def assert_ne[T](actual: T, expected: T)  where T: Debug + Eq
def fail(msg: &str) -> Never
```

Failure message format:

```
assertion failed: actual != expected
  actual:   Array([1, 2, 3])
  expected: Array([1, 2, 4])
  at tests/foo.rvn:12:5
```

v1 compromise: without macros, `assert_eq` cannot stringify the
expression text. Accept the slight degradation. v2 upgrades to
`assert_eq!(actual, expected)` with full stringification.

### 4.4 Output format

Default (`--format=pretty`, Rust-style):

```
running 5 tests
test tests.arithmetic.adds_two_numbers ... ok
test tests.arithmetic.subtracts ... ok
test tests.edge_cases.negative_overflow ... FAILED
test tests.edge_cases.skip_me ... skipped
test tests.edge_cases.panic_expected ... ok

failures:

---- tests.edge_cases.negative_overflow ----
assertion failed: actual != expected
  actual:   -1
  expected: 0
  at tests/edge_cases.rvn:15:5

test result: FAILED. 3 passed; 1 failed; 1 skipped; finished in 0.04s
```

TAP output (`--format=tap`):

```
TAP version 13
1..5
ok 1 - tests.arithmetic.adds_two_numbers
ok 2 - tests.arithmetic.subtracts
not ok 3 - tests.edge_cases.negative_overflow
  ---
  message: "assertion failed: actual != expected"
  ...
ok 4 - tests.edge_cases.skip_me # SKIP
ok 5 - tests.edge_cases.panic_expected
```

JSON output (`--format=json`): one event per line,
`{"type":"test","event":"ok","name":"..."}`. Schema-matches Rust's
`libtest --format=json` where practical.

### 4.5 Test discovery

- **Inline tests.** Any `def` in any `.rvn` file whose body contains a
  `test` directive (as the first statement).
- **Integration tests.** Files under `tests/**.rvn`. Each top-level
  `def` whose body contains `test` becomes a test.
- **Naming convention.** Test path is `<module-path>.<fn-name>`.
  For `src/math/helpers.rvn` with a `test`-directive `def adds_two`,
  the path is `math.helpers.adds_two`. For `tests/foo.rvn` with a
  `test`-directive `def bar`, the path is `tests.foo.bar`.

---

## 5. Architecture / design

### 5.1 Compiler changes

**Body-directive recognition.** The parser's `parse_func_body` needs to
recognize a small set of bare keywords (`test`, `bench`, `ignore`,
`should_panic`, `only`) at the head of a function body. Approach:
inspect the first non-whitespace, non-doc-comment statement of every
parsed `def`; if it's a bare identifier matching one of these keywords
(optionally followed by a literal argument), record it on the AST node
and consume.

```rust
// pseudocode
fn parse_func_body(&mut self) -> FuncBody {
    let mut directives = vec![];
    loop {
        if let Some(d) = self.try_parse_test_directive() {
            directives.push(d);
        } else {
            break;
        }
    }
    let stmts = self.parse_statements();
    FuncBody { directives, stmts }
}
```

This requires adding `test_directives: Vec<TestDirective>` to
`ast::FuncDef` and `hir::HirFuncDef` (currently these don't carry such
metadata). Also threads through `resolve/mod.rs` and the formatter.

**Typechecker.** No change needed — `test`-directive functions are
regular functions with signature `def name()` (no args, no return value
beyond Unit). A validator (new module
`crates/riven-core/src/typeck/test_check.rs`) emits a diagnostic if a
`test` directive is on a function with non-empty parameters or a
non-Unit/Never return type.

**MIR / codegen.** Unchanged — each test function lowers like any
other. The test binary is a separate codegen job that:

1. Takes the compiled `.o` files of the user's source + test files.
2. Generates a synthesized `main` (as MIR, lowered normally) that
   iterates over a static array of `(name: &str, fn: fn())` pairs.
3. Links everything together.

See §5.3 for the harness generation mechanics.

### 5.2 CLI plumbing — `riven test`

Add to `crates/riven-cli/src/cli.rs::Command`:

```rust
Test {
    #[arg(long)] release: bool,
    #[arg(long)] no_run: bool,
    #[arg(long = "test-threads", default_value = "auto")] test_threads: String,
    #[arg(long)] nocapture: bool,
    #[arg(long)] ignored: bool,
    #[arg(long = "include-ignored")] include_ignored: bool,
    #[arg(long)] exact: bool,
    #[arg(long)] list: bool,
    #[arg(long)] format: Option<String>,
    filter: Option<String>,
},
```

New module `crates/riven-cli/src/test.rs`:

```rust
pub fn test(opts: TestOptions) -> Result<(), String> {
    // 1. Discover test files:
    //    src/**.rvn + tests/**.rvn
    // 2. For each source file, compile with test-aware codegen that
    //    extracts functions whose bodies carry a `test` directive.
    // 3. Generate a harness main.rvn at target/riven/tests/harness.rvn
    // 4. Compile the harness + user code into target/debug/deps/test-<hash>.
    // 5. Unless --no-run, exec the binary with the filter passed through.
}
```

Output of the test binary is either parsed-and-re-rendered (to produce
TAP/JSON from the binary's native output) or the binary itself emits
in the requested format based on args it receives.

### 5.3 Harness generation

The harness is a generated `.rvn` file (or directly-generated MIR
function) that looks like:

```rvn
# Auto-generated; do not edit.
use std.test.internal.{TestCase, run_all}

def main
  let tests: Array[TestCase] = [
    TestCase.new("tests.arithmetic.adds_two_numbers", adds_two_numbers_0xabc),
    TestCase.new("tests.arithmetic.subtracts",         subtracts_0xdef),
    # ...
  ]
  run_all(tests)
end
```

Each `*_0xHASH` is the mangled name of a test function. The mangling
ensures no collision between `adds_two_numbers` in `tests/a.rvn` and
`adds_two_numbers` in `tests/b.rvn`.

`std.test.internal.run_all` is the runtime side: it parses
`argv`, applies the filter, runs each test (optionally in parallel),
catches panics via a signal handler (SIGABRT from `abort()` → jump to
test-fail handler), collects results, and prints.

**Panic catching.** The C runtime's `riven_panic` (`runtime.c:423-426`)
currently calls `abort()`. For test mode, the runtime must be compiled
with a variant that `longjmp`s back to the test runner. Simplest
approach: an environment variable `RIVEN_TEST_MODE=1` that makes
`riven_panic` call a special handler in the test runtime instead.

Alternative: fork-per-test. Every test runs in a child process. Crash
is just a non-zero exit code. Simpler, more robust, slower. **Recommend
this for v1** — fork is cheap on Linux/macOS and isolates leaking
tests. Move to in-process later.

### 5.4 Parallelism

With fork-per-test (recommended), parallelism is trivial: fork N at a
time. Default N = number of CPUs. `--test-threads=1` runs serially.

With in-process (v2), parallelism requires (a) tier-1 doc 02
(concurrency) and (b) thread-safe globals. Defer.

### 5.5 Filtering semantics

- Multiple positional filters OR'd: `riven test foo bar` runs tests
  whose path contains `foo` OR `bar`.
- `--exact` requires whole-path match.
- `--ignored` runs *only* ignored tests.
- `--include-ignored` runs both.

These semantics match `cargo test`.

### 5.6 Cache integration

The existing incremental cache (`rivenc/src/cache/`) already handles
per-file caching with a `flags` field in `BuildOptions`. Test builds
pass `flags="test"` to keep test objects separate from normal build
objects. No new cache logic needed.

---

## 6. Implementation plan

### Files to touch

| Phase | File | Change |
|---|---|---|
| 1 | `crates/riven-core/src/parser/ast.rs` (FuncDef) | Add `test_directives: Vec<TestDirective>` |
| 1 | `crates/riven-core/src/parser/mod.rs` (parse_func_body) | Recognize `test`/`ignore`/`should_panic`/`only`/`bench` at body head |
| 1 | `crates/riven-core/src/hir/nodes.rs` (HirFuncDef) | Add `test_directives: Vec<TestDirective>` |
| 1 | `crates/riven-core/src/resolve/mod.rs` | Thread directives from ast → hir |
| 1 | `crates/riven-core/src/typeck/test_check.rs` *new* | Validate `test`-directive functions have no params, return Unit |
| 1 | `crates/riven-core/src/formatter/format_items.rs` | Round-trip `test`-directive body in formatter |
| 2 | `crates/riven-cli/src/cli.rs:25-114` | Add `Test` command |
| 2 | `crates/riven-cli/src/test.rs` *new* | Discovery, harness gen, build, run (~400 lines) |
| 2 | `crates/riven-cli/src/main.rs:7-52` | Wire `Command::Test` |
| 2 | `crates/riven-cli/src/module_discovery.rs` | Extend to walk `tests/**.rvn` |
| 3 | `crates/riven-core/runtime/runtime.c` | Add `riven_test_panic` entry point (longjmp or plain exit) |
| 3 | `stdlib/test/mod.rvn` *new* | `TestCase`, `run_all`, `assert*` helpers (builtin registration first; real stdlib once tier-1 doc 01 lands) |
| 4 | `crates/riven-cli/src/scaffold.rs:29-50` | `riven new` scaffolds a `tests/hello.rvn` with one example |
| 4 | `docs/tutorial/17-testing.md` *new* | Tutorial page |

### Phase breakdown

**Phase 1 — Directive pipeline for `test` (3 days).**
Teach the parser, AST, HIR, resolver, formatter about body-head test
directives on `def`. Minimum scope: `test` + `ignore` + `should_panic`.
No other directives accepted (the general macro dispatcher lands with
tier-1 doc 05).

**Phase 2 — `riven test` subcommand (3 days).**
- Day 1: CLI + discovery (walk `src/**.rvn` + `tests/**.rvn`, find
  functions whose bodies contain a `test` directive).
- Day 2: harness generation; compile test binary.
- Day 3: fork-per-test runner; pretty output format.

**Phase 3 — Runner + assertions (2 days).**
- Day 1: `assert`, `assert_eq`, `assert_ne`, `fail` as compiler builtins
  (registered in `resolve::mod::register_builtins`).
- Day 2: TAP + JSON output; `--list`; filter semantics.

**Phase 4 — Polish (1 day).**
`riven new` scaffolds an example test; docs; diagnostic errors for
mis-used `test` directive.

Total: 9 engineer-days for a v1 that covers 90% of day-to-day usage.

---

## 7. Interactions with other tier-3 items

- **Doc 05 (bench).** Shares 90% of the harness: same discovery, same
  CLI skeleton, same output plumbing. Implement bench after test so
  the shared code in `riven-cli/src/test.rs` is factored cleanly.
  Expected extraction: `test_harness::{discover, build, run}` common
  to both.
- **Doc 04 (docs).** `rivendoc` should emit per-function test counts
  if any — a small integration point.
- **Doc 01 (LSP).** Add code-lens `"Run test"` / `"Debug test"`
  above each function with a `test` directive once LSP ships rename.
- **Doc 02 (debugger).** `riven test --debug foo.bar` launches the
  test binary under a debugger.
- **Doc 06 (incremental).** No test run should re-compile source files
  that are cached. Verify `cache` handles the `flags="test"` key.
- **Doc 08 (property).** Property tests would layer on top of this
  framework as a `proptest` directive alongside `test` — out of scope v1.

### Tier-1 dependencies

- **Tier-1 doc 05 (macros).** Real stringifying `assert_eq!`
  (macro) replaces the builtin function `assert_eq` in v2.
- **Tier-1 doc 01 (stdlib).** `fmt::Debug` is needed to print actual /
  expected values. Until it lands, v1 assertions render `actual = <opaque>`
  for non-primitive types.
- **Tier-1 doc 04 (Drop).** Every test allocation leaks today (B1).
  Long-running test suites will OOM CI. Ship the `test` directive only
  after Drop works for `String`/`Array` or document the leak loudly.

---

## 8. Phasing

| Phase | Scope | Days | Prereqs |
|---|---|---|---|
| 1 | Directive pipeline | 3 | None |
| 2 | `riven test` subcommand | 3 | 1 |
| 3 | Runner + assertions | 2 | 2 |
| 4 | Polish + docs | 1 | 3 |
| 5 | Macro-based assertions | + | tier-1 doc 05 |

---

## 9. Open questions & risks

1. **OQ-1 — Directive surface form.**
   Body-head bare-keyword (`test` on its own line, Ruby-style — matches
   `private`, `include`, `layout`). The prefix-attribute
   surface is retired entirely. Confirm the bare-keyword form.
2. **OQ-2 — `should_panic` string match semantics.**
   Substring of the panic message, regex, or exact match? Recommend
   substring (matches Rust's `should_panic(expected = "...")`).
3. **OQ-3 — `fail(msg)` return type `Never`.**
   Requires `Ty::Never` plumbed through the typechecker. Is it?
   (Quick check: `riven-core/src/hir/types.rs` has a `Never` variant;
   verify it's usable as function return type.) If not, use
   `-> Unit` with an internal never-returning marker.
4. **OQ-4 — Test path separator.**
   `tests.arith.adds` (Riven uses `.` everywhere — `std.io`,
   `package.utils`). Recommend `.` to match module-path conventions.
5. **OQ-5 — Default `--test-threads`.**
   Rust defaults to the number of CPUs. Riven should follow, but with
   fork-per-test this might oversubscribe. Limit to min(ncpus, 8) by
   default.
6. **R1 — Panic-catching without proper stack unwinding.**
   Riven has no unwinding runtime. Fork-per-test sidesteps this. If
   we ever move to in-process, we need `setjmp`/`longjmp` discipline
   and it must not interfere with Drop (tier-1 doc 04).
7. **R2 — Leaking tests.**
   Until tier-1 doc 04 lands, every `let s = String.from(...)` inside
   a test leaks. Acceptable with fork-per-test (child exits, OS
   reclaims) but documentably bad.
8. **R3 — Windows.**
   No `fork` on Windows. v1 is Linux+macOS only.
9. **OQ-6 — Filter path semantics for integration tests.**
   `tests/foo.rvn.bar` — is the file path part of the test path?
   Recommend `tests.foo.bar` (drop the extension, use `.`).
10. **OQ-7 — Test binary location.**
    `target/debug/deps/test-<hash>` (cargo-style) or
    `target/debug/test/<binary>`? Recommend the latter for
    discoverability.

---

## 10. Test matrix (meta — testing the test framework)

| Case | Expected |
|---|---|
| `def passing; test; ... end` returns Unit | Runner reports `ok` |
| `def failing; test; assert(false); end` | Runner reports `FAILED`, exit 1 |
| `def x; test; ignore; end` | Runner reports `skipped` |
| `def x; test; ignore "flaky"; end` | Skip reason shown in output |
| `def x; test; should_panic; panic!; end` | Reports `ok` |
| `def x; test; should_panic; ...; end` without panic | Reports `FAILED` |
| `def x(i: Int); test; ... end` | Compile error: test fns take no args |
| `def x -> Int; test; ...; end` | Compile error: return type must be Unit |
| `riven test foo` with no matches | Exit 0, prints "0 passed" |
| `riven test --exact foo.bar` matches exactly | Only that test runs |
| `riven test --list --format=json` | JSON event per test |
| `riven test --nocapture` | Test's `puts` reaches stdout |
| Panic in test | Reported as failure, other tests continue |
| Infinite loop in test | (v2) `--timeout` kills it; v1 no timeout |
| `assert_eq(1, 2)` | Prints "actual: 1 / expected: 2" with file:line |
| Integration test in `tests/foo.rvn` | Discovered |
| `riven test --release` | Rebuilds in release mode |
| Second `riven test` with no changes | Uses cache; fast |
| Forked test crashes via OOB access | Reported as failed, not a runner crash |

These live under `crates/riven-cli/tests/test_runner.rs` as
integration tests that invoke `riven test` on fixture projects in
`crates/riven-cli/tests/fixtures/tests-*/`.
