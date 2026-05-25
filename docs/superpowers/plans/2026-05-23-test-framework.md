# Test Framework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-druxen-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a pure-Ruxen `std.test` package + `ruxen test` CLI subcommand that discovers `tests/**.rx`, wraps each file in a synthesized `def main`, builds per-file binaries through the existing incremental cache, and runs them in parallel with fork-per-test isolation.

**Architecture:** `library/std/test/` exposes `Tester.describe(name) do |t| ... end` as the only class entry; nested `t.context` / `t.it` / `t.before` / `t.after` / `t.xit` / `t.expect` / `t.expect_panic` are instance methods on the yielded `Tester`. Discovery and synthesis happen in Rust (`src/ruxenc/src/test_runner.rs` + `src/ruxen_cli/src/test.rs`); execution is process-isolated per test case via a small `fork()` C shim in `library/std/test/runtime/test.c`.

**Tech Stack:** Ruxen (stdlib), Rust (CLI + ruxenc), C (3-function FFI shim for `fork`/`waitpid`/`current-runner` slot).

**Spec:** `docs/superpowers/specs/2026-05-23-test-framework-design.md`.

**Conventions (load-bearing — do not skip):**
- `feedback_no_inline_rx_in_pin_tests.md` — Ruxen source for tests lives in `compiler/ruxen_core/tests/fixtures/ruxen/<stem>.rx`, never inline `r#"..."#`.
- `feedback_no_full_paths_in_shell.md` — bare `git` / `cargo`, no `-C /Users/hassan/...`.
- `feedback_no_commit_co_authors.md` — strip the `Co-Authored-By:` trailer.
- `feedback_no_git_push.md` — commits OK; no `git push`, no PRs.
- `project_ruxen_bootstrap_files_load_check.md` — new stdlib packages MUST be added to `compiler/ruxen_core/src/resolve/bootstrap.rs` `BOOTSTRAP_FILES` or they will silently fail to load.
- Cache test runs to `tmp/test-cache/<name>.log` (rule 41).

---

## File Structure

**New files (Ruxen stdlib package):**
- `library/std/test/Ruxen.toml` — manifest (deps: std-core, std-sync, std-string, std-array, std-option_result, std-fmt)
- `library/std/test/src/lib.rx` — public entry: `Tester`, `Matcher`, `TestCase`, `Runner` re-exports + the small `bootstrap_smoke` symbol
- `library/std/test/src/tester.rx` — `class Tester` + `describe`/`context`/`it`/`xit`/`before`/`after` + `expect` / `expect_panic`
- `library/std/test/src/matcher.rx` — `class Matcher[T]` + matchers (`to_eq`, `not_to_eq`, `to_be_truthy`, `to_be_falsy`, `to_include`, `to_be_nil`, `not_to_be_nil`, `to_be_a`)
- `library/std/test/src/test_case.rx` — `class TestCase` (name + body closure + pending flag + expect-panic substring)
- `library/std/test/src/runner.rx` — `class Runner` (group tree, current-runner slot, `execute`)
- `library/std/test/runtime/test.c` — three C entry points: `ruxen_test_current_set` / `ruxen_test_current_get` (process-static `int64_t` slot for the active Runner handle) and `ruxen_test_fork_and_wait` (fork + run-child-closure + waitpid, returning child exit code)

**New files (Rust CLI):**
- `src/ruxenc/src/test_runner.rs` — discovery (walk `tests/**.rx`), synthesis wrap, build invocation, parallel dispatch, output rendering (~500 LOC)
- `src/ruxenc/src/test_output.rs` — pretty / TAP / JSON formatters (~150 LOC)
- `src/ruxen_cli/tests/fixtures/test-projects/basic/` — integration-test fixture project (Ruxen.toml + tests/example.rx)
- `src/ruxen_cli/tests/test_runner.rs` — integration tests for the `ruxen test` end-to-end flow

**Modified files:**
- `compiler/ruxen_core/src/resolve/bootstrap.rs:77-162` — append `"test/src/lib.rx"` to `BOOTSTRAP_FILES`
- `src/ruxenc/src/lib.rs` — `pub mod test_runner; pub mod test_output;`
- `src/ruxen_cli/src/cli.rs:174-179` — replace placeholder `Test { filter }` with full subcommand definition
- `src/ruxen_cli/src/main.rs:116-122` — replace `exit(2)` stub with `ruxenc::test_runner::run(...)`
- `src/ruxen_cli/src/scaffold.rs` — add a `tests/example.rx` to the `ruxen new` template

**New test files (TDD harness):**
- `compiler/ruxen_core/tests/stdlib_test.rs` — Rust-side integration tests that compile + run `.rx` fixtures exercising the `std.test` API
- `compiler/ruxen_core/tests/fixtures/ruxen/test_*.rx` — fixture Ruxen programs

**New docs:**
- `docs/tutorial/17-testing.md` — user-facing tutorial page

---

## Phase 1 — Package skeleton and bootstrap wiring

Goal: an empty-but-loaded `std.test` package that the bootstrap merger recognizes. Catches the `BOOTSTRAP_FILES` orphan-file class of bugs (memory `project_ruxen_bootstrap_files_load_check.md`) before we sink time into class bodies.

### Task 1.1: Create the package manifest

**Files:**
- Create: `library/std/test/Ruxen.toml`

- [ ] **Step 1: Write the manifest**

```toml
[package]
name = "std-test"
version = "0.1.0"
description = "Pure-Ruxen test framework: Tester DSL (describe/context/it/before/after) + matchers + fork-per-test runner. The ruxen test CLI subcommand drives it. See docs/superpowers/specs/2026-05-23-test-framework-design.md."

[dependencies]
std-core = "= 0.1.0"
std-string = "= 0.1.0"
std-array = "= 0.1.0"
std-option_result = "= 0.1.0"
std-fmt = "= 0.1.0"
std-sync = "= 0.1.0"
```

- [ ] **Step 2: Commit**

```bash
git add library/std/test/Ruxen.toml
git commit -m "stdlib(test): manifest for std-test package"
```

### Task 1.2: Create a minimal lib.rx that loads cleanly

**Files:**
- Create: `library/std/test/src/lib.rx`

- [ ] **Step 1: Write a single-symbol stub so the file is parseable**

```rx
## std::test — pure-Ruxen test framework (skeleton).
##
## Public surface ships in companion files (loaded via bootstrap order):
##   tester.rx      — `class Tester` + describe/context/it/before/after
##   matcher.rx     — `class Matcher[T]` + v1 matchers
##   test_case.rx   — `class TestCase` (name + body + pending flag)
##   runner.rx      — `class Runner` + current-runner slot + execute
##
## This file is the BOOTSTRAP entry; it carries one marker symbol so the
## bootstrap pre-walk has something to register.
class StdTestPackageMarker
  layout c
  marker: Int

  def init
    self.marker = 1
  end
end
```

- [ ] **Step 2: Commit**

```bash
git add library/std/test/src/lib.rx
git commit -m "stdlib(test): bootstrap-loadable lib.rx stub"
```

### Task 1.3: Wire the package into BOOTSTRAP_FILES

**Files:**
- Modify: `compiler/ruxen_core/src/resolve/bootstrap.rs:77-162` (append before the closing `];`)

- [ ] **Step 1: Add the entry**

Find the existing `bench/src/lib.rx,` line near the end of `BOOTSTRAP_FILES` and add immediately after it:

```rust
    // test — pure-Ruxen test framework (Tester DSL + Matcher + Runner).
    // Depends on string/array/option_result/fmt/sync — all already loaded
    // above. Discovery + synthesis live in Rust (ruxenc::test_runner);
    // this entry registers the runtime classes the synthesised `def main`
    // references via `use std.test.Tester` / `use std.test.Runner`.
    "test/src/lib.rx",
```

- [ ] **Step 2: Run the bootstrap-parse pin test**

```bash
mkdir -p tmp/test-cache
cargo test -p ruxen_core --test bootstrap_prelude_merge 2>&1 | tee tmp/test-cache/bootstrap-after-test-entry.log
```

Expected: PASS (all bootstrap files parse cleanly; package count incremented by 1).

- [ ] **Step 3: Commit**

```bash
git add compiler/ruxen_core/src/resolve/bootstrap.rs
git commit -m "stdlib(test): register std-test in BOOTSTRAP_FILES"
```

---

## Phase 2 — Test data classes (TestCase + Matcher)

Goal: the value types the DSL produces. No execution semantics yet — just data carriers that pass typeck.

### Task 2.1: `class TestCase` — name + body + pending + expect_panic

**Files:**
- Create: `library/std/test/src/test_case.rx`
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_case_construct.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (new file)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_case_construct.rx`:

```rx
use std.test.TestCase

def main
  let tc = TestCase.new("adds two numbers", { || puts "ran" })
  puts "name=#{tc.name}"
  puts "pending=#{tc.pending}"
end
```

- [ ] **Step 2: Write the failing Rust test harness**

`compiler/ruxen_core/tests/stdlib_test.rs`:

```rust
//! Integration tests for the `std.test` stdlib package.
//!
//! Fixture .rx files live under `tests/fixtures/ruxen/test_*.rx` per
//! the no-inline-rx convention (feedback_no_inline_rx_in_pin_tests.md).
//! Each test compiles + runs a fixture and asserts on captured stdout.

use ruxen_core::codegen;
use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn test_case_construct_name_and_pending_default_false() {
    let (stdout, stderr, ok) =
        compile_and_run(&rx("test_case_construct"), "stdlib_test_case_construct");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("name=adds two numbers"), "got: {}", stdout);
    assert!(stdout.contains("pending=false"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run the test — expect failure (TestCase not defined)**

```bash
cargo test -p ruxen_core --test stdlib_test test_case_construct 2>&1 | tee tmp/test-cache/2.1-red.log
```

Expected: FAIL — `use std.test.TestCase` cannot resolve (`TestCase` is not yet defined).

- [ ] **Step 4: Implement TestCase**

`library/std/test/src/test_case.rx`:

```rx
## std.test.TestCase — one executable test case.
##
## Built by `Tester#it(name) do ... end` and `Tester#xit(name) do ... end`.
## The Runner walks the group tree (see runner.rx) and invokes each
## TestCase's body closure inside a forked child process for isolation.
##
## `expect_panic_substr` is None for an ordinary test. When set, the
## runner inverts the success condition: the child MUST panic and the
## captured stderr MUST contain the substring.

class TestCase
  name: String
  body: any Fn() -> ()
  pending: Bool
  expect_panic_substr: String?

  def init(name: &String, body: any Fn() -> ())
    self.name = name.clone
    self.body = body
    self.pending = false
    self.expect_panic_substr = nil
  end
end
```

- [ ] **Step 5: Run the test — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test test_case_construct 2>&1 | tee tmp/test-cache/2.1-green.log
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/test_case.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_case_construct.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): TestCase value class — name + body + pending"
```

### Task 2.2: `class Matcher[T]` — `to_eq` and `not_to_eq`

**Files:**
- Create: `library/std/test/src/matcher.rx`
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_to_eq.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_to_eq.rx`:

```rx
use std.test.Matcher

def main
  let m = Matcher.new(1 + 2)
  if m.to_eq(3)
    puts "to_eq_pass"
  else
    puts "to_eq_fail"
  end

  let n = Matcher.new(1 + 2)
  if n.not_to_eq(4)
    puts "not_to_eq_pass"
  else
    puts "not_to_eq_fail"
  end
end
```

- [ ] **Step 2: Append failing Rust test**

In `compiler/ruxen_core/tests/stdlib_test.rs`:

```rust
#[test]
fn matcher_to_eq_and_not_to_eq() {
    let (stdout, stderr, ok) =
        compile_and_run(&rx("test_matcher_to_eq"), "stdlib_test_matcher_to_eq");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("to_eq_pass"), "got: {}", stdout);
    assert!(stdout.contains("not_to_eq_pass"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run — expect failure**

```bash
cargo test -p ruxen_core --test stdlib_test matcher_to_eq_and_not_to_eq 2>&1 | tee tmp/test-cache/2.2-red.log
```

Expected: FAIL — `Matcher` unresolved.

- [ ] **Step 4: Implement Matcher with `to_eq` / `not_to_eq`**

`library/std/test/src/matcher.rx`:

```rx
## std.test.Matcher[T] — value wrapper returned by `Tester#expect(x)`.
##
## v1 matchers are pure-boolean: `to_eq` / `not_to_eq` etc. return Bool;
## the Tester layer turns a `false` into a `ruxen_panic` with a
## structured message the Runner parent decodes.
##
## Equality matchers require `T: PartialEq`. Non-PartialEq types fail
## typeck at the call site (correct — surfaces the constraint early).

class Matcher[T]
  where T: PartialEq

  actual: T

  def init(actual: T)
    self.actual = actual
  end

  def to_eq(expected: T) -> Bool
    self.actual == expected
  end

  def not_to_eq(expected: T) -> Bool
    self.actual != expected
  end
end
```

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test matcher_to_eq_and_not_to_eq 2>&1 | tee tmp/test-cache/2.2-green.log
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/matcher.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_to_eq.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): Matcher[T] with to_eq / not_to_eq"
```

### Task 2.3: Matcher truthy/falsy/nil checks

**Files:**
- Modify: `library/std/test/src/matcher.rx`
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_truthy_nil.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_truthy_nil.rx`:

```rx
use std.test.Matcher

def main
  let m_true = Matcher.new(true)
  if m_true.to_be_truthy
    puts "truthy_pass"
  end

  let m_false = Matcher.new(false)
  if m_false.to_be_falsy
    puts "falsy_pass"
  end

  let opt_some: Int? = 7
  let m_some = Matcher.new(opt_some)
  if m_some.not_to_be_nil
    puts "not_nil_pass"
  end

  let opt_none: Int? = nil
  let m_none = Matcher.new(opt_none)
  if m_none.to_be_nil
    puts "nil_pass"
  end
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn matcher_truthy_falsy_and_nil() {
    let (stdout, stderr, ok) =
        compile_and_run(&rx("test_matcher_truthy_nil"), "stdlib_test_matcher_truthy_nil");
    assert!(ok, "stderr: {}", stderr);
    for token in ["truthy_pass", "falsy_pass", "not_nil_pass", "nil_pass"] {
        assert!(stdout.contains(token), "missing {token}: {}", stdout);
    }
}
```

- [ ] **Step 3: Run — expect failure** (`to_be_truthy` not defined)

```bash
cargo test -p ruxen_core --test stdlib_test matcher_truthy_falsy_and_nil 2>&1 | tee tmp/test-cache/2.3-red.log
```

- [ ] **Step 4: Extend Matcher**

Append to `library/std/test/src/matcher.rx` AFTER the `class Matcher[T]` block; for the nullable matchers we add a separate, non-generic-constrained sibling class because the `where T: PartialEq` constraint on the generic Matcher would block `Matcher.new(opt)` for Option-typed values that may not implement PartialEq.

```rx
## Boolean-valued matcher path. Decoupled from the generic Matcher[T]
## constraint because `to_be_truthy`/`to_be_falsy` don't need equality
## and the call site shouldn't be forced to satisfy PartialEq for a
## boolean assertion.
class BoolMatcher
  actual: Bool

  def init(actual: Bool)
    self.actual = actual
  end

  def to_be_truthy -> Bool
    self.actual
  end

  def to_be_falsy -> Bool
    self.actual == false
  end
end

## Nilness check for any Option-typed value. We accept the value as
## Int? at the v1 surface because the test framework only inspects
## "is present?" — the inner type is irrelevant. The Tester#expect
## overload (Tester#expect_opt) routes Option-typed arguments here.
class OptionMatcher
  is_some: Bool

  def init(present: Bool)
    self.is_some = present
  end

  def to_be_nil -> Bool
    self.is_some == false
  end

  def not_to_be_nil -> Bool
    self.is_some
  end
end
```

And extend the generic Matcher to provide bool/nil shortcuts when the call site stays on the equality-path: add to the `class Matcher[T]` body, **inside the existing `where T: PartialEq` block**:

```rx
  def to_be_truthy -> Bool
    # Generic to_be_truthy is awkward without a Truthy mixin; v1
    # routes Bool callers to BoolMatcher.new explicitly. This stub
    # exists so the Tester layer can call .to_be_truthy uniformly
    # when the typeck pass has already narrowed T to Bool — currently
    # unused; remove after Tester is wired.
    false
  end
```

NOTE: the fixture uses `Matcher.new(true)` directly — that compiles only if `Bool: PartialEq` (true for primitives). If `Bool: PartialEq` isn't in the prelude yet, switch the fixture to use `BoolMatcher.new(true).to_be_truthy` explicitly:

```rx
use std.test.BoolMatcher
use std.test.OptionMatcher

def main
  if BoolMatcher.new(true).to_be_truthy
    puts "truthy_pass"
  end
  if BoolMatcher.new(false).to_be_falsy
    puts "falsy_pass"
  end
  if OptionMatcher.new(true).not_to_be_nil
    puts "not_nil_pass"
  end
  if OptionMatcher.new(false).to_be_nil
    puts "nil_pass"
  end
end
```

Use the explicit class form in the fixture; the unified `expect` overload is built in Phase 3 once we know which path each argument-shape needs.

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test matcher_truthy_falsy_and_nil 2>&1 | tee tmp/test-cache/2.3-green.log
```

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/matcher.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_truthy_nil.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): BoolMatcher + OptionMatcher truthy/falsy/nil"
```

### Task 2.4: Matcher `to_include` for Array and String

**Files:**
- Modify: `library/std/test/src/matcher.rx`
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_include.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_include.rx`:

```rx
use std.test.ArrayMatcher
use std.test.StringMatcher

def main
  let xs = [1, 2, 3]
  if ArrayMatcher.new(&xs).to_include(2)
    puts "array_include_pass"
  end

  let s = "hello world"
  if StringMatcher.new(&s).to_include("world")
    puts "string_include_pass"
  end
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn matcher_to_include_array_and_string() {
    let (stdout, stderr, ok) =
        compile_and_run(&rx("test_matcher_include"), "stdlib_test_matcher_include");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("array_include_pass"), "got: {}", stdout);
    assert!(stdout.contains("string_include_pass"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run — expect failure**

```bash
cargo test -p ruxen_core --test stdlib_test matcher_to_include_array_and_string 2>&1 | tee tmp/test-cache/2.4-red.log
```

- [ ] **Step 4: Implement the two include matchers**

Append to `library/std/test/src/matcher.rx`:

```rx
## Element-membership matcher for Array[T]. Loops with `each` rather
## than `contains?` because the v1 Array surface may not expose contains?
## uniformly across T (PartialEq required).
class ArrayMatcher[T]
  where T: PartialEq

  actual_ref: &Array[T]

  def init(actual: &Array[T])
    self.actual_ref = actual
  end

  def to_include(needle: T) -> Bool
    var found = false
    self.actual_ref.each do |item|
      if item == needle
        found = true
      end
    end
    found
  end
end

## Substring matcher for String. Uses the v1 `contains` method exposed
## by std.string (verify name; if absent, port `String#contains` first).
class StringMatcher
  actual_ref: &String

  def init(actual: &String)
    self.actual_ref = actual
  end

  def to_include(needle: &String) -> Bool
    self.actual_ref.contains(needle)
  end
end
```

If `String.contains` doesn't exist yet, sub in a tiny inline scan:

```rx
  def to_include(needle: &String) -> Bool
    let haystack = self.actual_ref
    let h_len = haystack.len
    let n_len = needle.len
    if n_len > h_len
      return false
    end
    var i: Int = 0
    while i <= h_len - n_len
      if haystack.slice(i, i + n_len) == needle.clone
        return true
      end
      i = i + 1
    end
    false
  end
```

(`slice` / `clone` may need substitution depending on what `library/std/string/src/lib.rx` exposes — check before implementing.)

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test matcher_to_include_array_and_string 2>&1 | tee tmp/test-cache/2.4-green.log
```

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/matcher.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_matcher_include.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): ArrayMatcher + StringMatcher to_include"
```

---

## Phase 3 — Runner state slot (C shim)

Goal: a process-static slot for the active Runner handle, so `Tester.describe` can find "which Runner do I attach to?" without parameter-threading. Single mutable cell, no concurrency.

### Task 3.1: Runtime C shim — current-runner slot

**Files:**
- Create: `library/std/test/runtime/test.c`
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_current_runner_slot.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_current_runner_slot.rx`:

```rx
use std.test.Runner

def main
  Runner.set_current(42)
  let h = Runner.get_current
  puts "slot=#{h}"
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn runner_current_slot_roundtrip() {
    let (stdout, stderr, ok) =
        compile_and_run(&rx("test_current_runner_slot"), "stdlib_test_current_slot");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("slot=42"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run — expect failure** (`Runner` doesn't exist yet)

```bash
cargo test -p ruxen_core --test stdlib_test runner_current_slot_roundtrip 2>&1 | tee tmp/test-cache/3.1-red.log
```

- [ ] **Step 4: Write the C shim**

`library/std/test/runtime/test.c`:

```c
/* std.test runtime — three entry points only:
 *
 *   ruxen_test_current_set / ruxen_test_current_get
 *     Process-static slot holding the active Runner handle (an int64
 *     pointer cast to int64_t). Set by Runner.new before the user file
 *     body runs; read by Tester.describe to know which Runner to
 *     attach new root groups to. Single-thread access only —
 *     a test binary's DSL-setup phase is strictly single-threaded.
 *
 *   ruxen_test_fork_and_wait (Task 5.1)
 *     fork() + child runs a Ruxen closure + exit + parent waitpid.
 *     Lives in this file; Phase 5 fills the body.
 */

#include <stdint.h>

#include "../../core/runtime/runtime.h"

static int64_t ruxen_test_current_runner = 0;

int64_t ruxen_test_current_set(int64_t handle) {
    int64_t prev = ruxen_test_current_runner;
    ruxen_test_current_runner = handle;
    return prev;
}

int64_t ruxen_test_current_get(void) {
    return ruxen_test_current_runner;
}
```

- [ ] **Step 5: Write the Runner shell with the static accessors**

`library/std/test/src/runner.rx`:

```rx
## std.test.Runner — owns the group tree for one test FILE.
##
## Constructed by the synthesised `def main` (see ruxen test in
## `src/ruxenc/src/test_runner.rs`); the file body's `Tester.describe`
## calls attach root groups to the Runner via the process-static
## current-runner slot in runtime/test.c.

class Runner
  layout c
  handle: Int   # opaque future use — Runner is heap-resident, this is self-ptr
  name: String

  def init(test_path: &String)
    self.handle = 0
    self.name = test_path.clone
  end

  lib "runtime/test.c"
    def self.set_current as "ruxen_test_current_set"(handle: Int) -> Int
    def self.get_current as "ruxen_test_current_get"() -> Int
  end
end
```

- [ ] **Step 6: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test runner_current_slot_roundtrip 2>&1 | tee tmp/test-cache/3.1-green.log
```

- [ ] **Step 7: Commit**

```bash
git add library/std/test/runtime/test.c \
        library/std/test/src/runner.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_current_runner_slot.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): Runner shell + process-static current-runner slot"
```

---

## Phase 4 — Tester DSL (sequential, in-process execution)

Goal: the user-facing DSL works end-to-end with in-process sequential execution. No fork yet, no parallelism — just `describe → context → it → expect`, drained at end-of-file by `Runner#execute` which prints pass/fail to stdout.

### Task 4.1: `class Tester` — minimal `describe → it → expect.to_eq`

**Files:**
- Create: `library/std/test/src/tester.rx`
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_tester_describe_it_eq.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_tester_describe_it_eq.rx`:

```rx
use std.test.Tester
use std.test.Runner

def main
  let r = Runner.new("fixture.describe_it")
  Runner.set_current(r.handle_addr)

  Tester.describe("Calculator") do |t|
    t.it("adds two numbers") do
      let m = t.expect(1 + 2)
      if m.to_eq(3) == false
        puts "ASSERT_FAIL_1+2_should_be_3"
      end
    end
    t.it("subtracts") do
      let m = t.expect(5 - 2)
      if m.to_eq(3) == false
        puts "ASSERT_FAIL_5-2_should_be_3"
      end
    end
  end

  r.execute
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn tester_describe_it_expect_to_eq_pass_path() {
    let (stdout, stderr, ok) = compile_and_run(
        &rx("test_tester_describe_it_eq"),
        "stdlib_test_tester_describe_it_eq",
    );
    assert!(ok, "stderr: {}", stderr);
    // No ASSERT_FAIL_* lines should appear (both assertions pass).
    assert!(!stdout.contains("ASSERT_FAIL"), "unexpected fail: {}", stdout);
    // Runner.execute should report 2 passing cases.
    assert!(stdout.contains("2 passed"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run — expect failure**

```bash
cargo test -p ruxen_core --test stdlib_test tester_describe_it_expect_to_eq_pass_path 2>&1 | tee tmp/test-cache/4.1-red.log
```

- [ ] **Step 4: Implement Tester + Runner.execute**

`library/std/test/src/tester.rx`:

```rx
use std.test.TestCase
use std.test.Matcher
use std.test.Runner

## std.test.Tester — one group in the spec tree.
##
## A root Tester is created by `Tester.describe`; nested Testers by
## `t.context`. Each Tester owns its child groups, its TestCases, and
## its before/after hooks. The Runner walks the tree at `execute` time.

class Tester
  name: String
  parent_handle: Int   # 0 if root, else address of parent Tester
  cases: Array[TestCase]
  children: Array[Tester]
  hooks_before: Array[any Fn() -> ()]
  hooks_after: Array[any Fn() -> ()]

  def init(name: &String, parent_handle: Int)
    self.name = name.clone
    self.parent_handle = parent_handle
    self.cases = Array.new
    self.children = Array.new
    self.hooks_before = Array.new
    self.hooks_after = Array.new
  end

  ## Class entry point: build a root group, run its body to populate it,
  ## then attach to the active Runner via the C-side current-runner slot.
  def self.describe(name: &String, body: any Fn(t: &var Tester) -> ()) -> ()
    var root = Tester.new(name, 0)
    body.(&var root)
    let runner_addr = Runner.get_current
    if runner_addr != 0
      let r: &var Runner = Runner.from_handle(runner_addr)
      r.attach_root(root)
    end
  end

  def context(name: &String, body: any Fn(t: &var Tester) -> ()) -> ()
    var child = Tester.new(name, 0)
    body.(&var child)
    self.children.push(child)
  end

  def it(name: &String, body: any Fn() -> ()) -> ()
    let tc = TestCase.new(name, body)
    self.cases.push(tc)
  end

  def xit(name: &String, body: any Fn() -> ()) -> ()
    var tc = TestCase.new(name, body)
    tc.pending = true
    self.cases.push(tc)
  end

  def before(body: any Fn() -> ()) -> ()
    self.hooks_before.push(body)
  end

  def after(body: any Fn() -> ()) -> ()
    self.hooks_after.push(body)
  end

  def expect[T](actual: T) -> Matcher[T]
    where T: PartialEq
    Matcher.new(actual)
  end
end
```

Extend `library/std/test/src/runner.rx`:

```rx
use std.test.Tester

class Runner
  layout c
  handle: Int
  name: String
  roots: Array[Tester]

  def init(test_path: &String)
    self.handle = 0
    self.name = test_path.clone
    self.roots = Array.new
  end

  ## Address of self, used to fish the Runner back out of the C-static
  ## current-runner slot. `__addr_of_self` is an existing intrinsic;
  ## if absent, expose via runtime/test.c ruxen_test_box_runner(handle).
  def handle_addr -> Int
    __addr_of_self
  end

  def attach_root(root: Tester) -> ()
    self.roots.push(root)
  end

  def execute -> ()
    var passed: Int = 0
    var failed: Int = 0
    var pending: Int = 0
    for root in &self.roots
      Runner.run_group(root, "", &var passed, &var failed, &var pending)
    end
    puts "#{passed} passed, #{failed} failed, #{pending} pending"
  end

  ## Recursive walker. `path` accumulates "describe > context > ...".
  def self.run_group(g: &Tester, path: &String,
                     passed: &var Int, failed: &var Int, pending: &var Int) -> ()
    let new_path = if path.len == 0
      g.name.clone
    else
      "#{path} > #{g.name}"
    end
    for case in &g.cases
      if case.pending
        pending = pending + 1
      else
        # In-process; panic in body aborts the binary (Phase 5 isolates).
        for hook in &g.hooks_before
          hook.()
        end
        case.body.()
        for hook in &g.hooks_after
          hook.()
        end
        passed = passed + 1
      end
    end
    for child in &g.children
      Runner.run_group(child, &new_path, &var passed, &var failed, &var pending)
    end
  end

  lib "runtime/test.c"
    def self.set_current as "ruxen_test_current_set"(handle: Int) -> Int
    def self.get_current as "ruxen_test_current_get"() -> Int
  end

  ## Recovers a &var Runner from a stored handle. Implemented as a thin
  ## intrinsic; if `__from_addr[T]` does not exist, expose via
  ## runtime/test.c with a typed wrapper. Tracking note: this is the
  ## one spot where the Tester layer needs to upcast an Int back to a
  ## Ruxen object; if the language can't, the alternative is to make
  ## the Runner slot store the box itself rather than a handle —
  ## that means a static Ruxen variable (which we'd then need a small
  ## std.sync OnceCell-style primitive for).
  def self.from_handle(addr: Int) -> &var Runner
    __from_addr[Runner](addr)
  end
end
```

**IMPLEMENTATION NOTE (open risk):** `__addr_of_self` and `__from_addr[T]` may not be language intrinsics. If not, plan-time fix: store the active Runner in a Ruxen-level static cell. Ruxen doesn't have static vars today either. The fallback is a small C-side handle table:

`library/std/test/runtime/test.c` additions:

```c
/* Single-slot Runner storage. We hold the box pointer itself, not a
 * cast handle, because the Ruxen side can pass us the box pointer
 * via a typed lib decl. */
static void *ruxen_test_runner_box = NULL;

int64_t ruxen_test_runner_set(int64_t box_ptr) {
    ruxen_test_runner_box = (void *)box_ptr;
    return 0;
}

int64_t ruxen_test_runner_get(void) {
    return (int64_t)ruxen_test_runner_box;
}
```

And in Ruxen, the Runner exposes:

```rx
lib "runtime/test.c"
  def self.box_store as "ruxen_test_runner_set"(box: Int) -> Int
  def self.box_load  as "ruxen_test_runner_get"() -> Int
end
```

Then `Runner.new(...)` calls `Runner.box_store(self_as_int)`; `Tester.describe` calls `Runner.box_load → cast back`. This works because Ruxen heap objects ARE pointers at the FFI boundary (per the std.bench precedent and the atomic shim's pattern of returning Int handles).

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test tester_describe_it_expect_to_eq_pass_path 2>&1 | tee tmp/test-cache/4.1-green.log
```

If the test fails on `__addr_of_self` / `__from_addr` resolution: switch to the C-slot fallback above, regenerate the fixture, re-run.

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/tester.rx \
        library/std/test/src/runner.rx \
        library/std/test/runtime/test.c \
        compiler/ruxen_core/tests/fixtures/ruxen/test_tester_describe_it_eq.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): Tester DSL + Runner.execute (in-process, sequential)"
```

### Task 4.2: `context` + `before` / `after` semantics

**Files:**
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_tester_context_hooks.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)
- Modify: `library/std/test/src/runner.rx` (`run_group` needs to inherit parent hooks)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_tester_context_hooks.rx`:

```rx
use std.test.Tester
use std.test.Runner

def main
  let r = Runner.new("fixture.context_hooks")
  Runner.box_store(r.handle_addr)

  Tester.describe("Outer") do |t|
    t.before do
      puts "outer_before"
    end
    t.after do
      puts "outer_after"
    end

    t.it("outer-case") do
      puts "outer_case_body"
    end

    t.context("Inner") do |t|
      t.before do
        puts "inner_before"
      end
      t.it("inner-case") do
        puts "inner_case_body"
      end
    end
  end

  r.execute
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn tester_context_inherits_parent_hooks() {
    let (stdout, stderr, ok) = compile_and_run(
        &rx("test_tester_context_hooks"),
        "stdlib_test_tester_context_hooks",
    );
    assert!(ok, "stderr: {}", stderr);
    // outer case sees only outer hooks (in order):
    let outer_idx = stdout.find("outer_case_body").expect("outer body");
    let outer_before = stdout[..outer_idx].find("outer_before").expect("outer_before before body");
    assert!(outer_before < outer_idx);
    // inner case sees outer_before THEN inner_before, then body, then outer_after:
    let inner_body_idx = stdout.find("inner_case_body").expect("inner body");
    let inner_outer_before = stdout[..inner_body_idx].rfind("outer_before").expect("outer_before for inner case");
    let inner_inner_before = stdout[..inner_body_idx].rfind("inner_before").expect("inner_before for inner case");
    assert!(inner_outer_before < inner_inner_before);
    assert!(inner_inner_before < inner_body_idx);
    // 2 passing, 0 failing, 0 pending
    assert!(stdout.contains("2 passed, 0 failed, 0 pending"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run — expect failure** (run_group doesn't pass parent hooks)

```bash
cargo test -p ruxen_core --test stdlib_test tester_context_inherits_parent_hooks 2>&1 | tee tmp/test-cache/4.2-red.log
```

- [ ] **Step 4: Update run_group to inherit hooks**

Replace `Runner.run_group` body in `library/std/test/src/runner.rx`:

```rx
  def self.run_group(g: &Tester,
                     path: &String,
                     inherited_before: &Array[any Fn() -> ()],
                     inherited_after: &Array[any Fn() -> ()],
                     passed: &var Int, failed: &var Int, pending: &var Int) -> ()
    let new_path = if path.len == 0
      g.name.clone
    else
      "#{path} > #{g.name}"
    end

    # Effective hook order for cases in THIS group:
    #   outermost_before → ... → this_before → body → this_after → ... → outermost_after
    var before_chain: Array[any Fn() -> ()] = Array.new
    for h in inherited_before
      before_chain.push(h)
    end
    for h in &g.hooks_before
      before_chain.push(h)
    end
    var after_chain: Array[any Fn() -> ()] = Array.new
    for h in &g.hooks_after
      after_chain.push(h)
    end
    for h in inherited_after
      after_chain.push(h)
    end

    for case in &g.cases
      if case.pending
        pending = pending + 1
      else
        for hook in &before_chain
          hook.()
        end
        case.body.()
        for hook in &after_chain
          hook.()
        end
        passed = passed + 1
      end
    end

    for child in &g.children
      Runner.run_group(child, &new_path, &before_chain, &after_chain,
                       &var passed, &var failed, &var pending)
    end
  end
```

Update `Runner.execute` to seed empty hook arrays:

```rx
  def execute -> ()
    var passed: Int = 0
    var failed: Int = 0
    var pending: Int = 0
    let empty: Array[any Fn() -> ()] = Array.new
    for root in &self.roots
      Runner.run_group(root, "", &empty, &empty,
                       &var passed, &var failed, &var pending)
    end
    puts "#{passed} passed, #{failed} failed, #{pending} pending"
  end
```

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test tester_context_inherits_parent_hooks 2>&1 | tee tmp/test-cache/4.2-green.log
```

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/runner.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_tester_context_hooks.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): context inherits parent before/after hooks"
```

### Task 4.3: `xit` reports pending; `it` failure increments `failed`

**Files:**
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_tester_xit_and_fail.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)
- Modify: `library/std/test/src/runner.rx` (failure detection — see step 4)
- Modify: `library/std/test/src/tester.rx` (`expect` failure path)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_tester_xit_and_fail.rx`:

```rx
use std.test.Tester
use std.test.Runner

def main
  let r = Runner.new("fixture.xit_and_fail")
  Runner.box_store(r.handle_addr)

  Tester.describe("Mixed") do |t|
    t.it("passes") do
      t.expect(1).to_eq(1)
    end
    t.it("fails") do
      t.expect(1).to_eq(2)
    end
    t.xit("pending") do
      t.expect(1).to_eq(99)
    end
  end

  r.execute
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn tester_summary_counts_pass_fail_pending() {
    let (stdout, stderr, _ok) = compile_and_run(
        &rx("test_tester_xit_and_fail"),
        "stdlib_test_tester_xit_and_fail",
    );
    // Binary may exit non-zero because one test failed — that's expected.
    assert!(stdout.contains("1 passed, 1 failed, 1 pending"),
            "got stdout={} stderr={}", stdout, stderr);
}
```

- [ ] **Step 3: Run — expect failure** (current code counts every non-pending as passed; nothing detects failure)

```bash
cargo test -p ruxen_core --test stdlib_test tester_summary_counts_pass_fail_pending 2>&1 | tee tmp/test-cache/4.3-red.log
```

- [ ] **Step 4: Wire `expect` failures into the count**

The simplest in-process path: `Tester#expect(...).to_eq(...)` should `ruxen_panic` when the comparison fails; the runner wraps each case body in a fork (Phase 5) to catch the abort.

In v1 sequential mode (no fork yet), use a per-case `bool` flag set by a class-method `Tester.mark_failure()` that the Matcher invokes on a failing comparison. Since Matcher doesn't know about Tester, do this via a per-process static failure flag:

Add to `library/std/test/runtime/test.c`:

```c
/* Per-case failure flag. Reset before each case body, set by any
 * failing matcher path. Reads as 0=pass, 1=fail. */
static int64_t ruxen_test_case_failed = 0;

int64_t ruxen_test_case_reset(void) {
    ruxen_test_case_failed = 0;
    return 0;
}

int64_t ruxen_test_case_mark_failed(void) {
    ruxen_test_case_failed = 1;
    return 0;
}

int64_t ruxen_test_case_get_failed(void) {
    return ruxen_test_case_failed;
}
```

Extend `library/std/test/src/runner.rx`:

```rx
  lib "runtime/test.c"
    def self.box_store as "ruxen_test_runner_set"(box: Int) -> Int
    def self.box_load  as "ruxen_test_runner_get"() -> Int
    def self.case_reset as "ruxen_test_case_reset"() -> Int
    def self.case_mark_failed as "ruxen_test_case_mark_failed"() -> Int
    def self.case_get_failed as "ruxen_test_case_get_failed"() -> Int
  end
```

Change `Matcher#to_eq` (and `not_to_eq`) in `library/std/test/src/matcher.rx` to set the flag on failure rather than returning bool — the user-facing call site doesn't need the bool anymore once Tester drives:

```rx
  def to_eq(expected: T) -> ()
    if self.actual != expected
      Runner.case_mark_failed
      puts "FAILED: expected #{expected.to_display}, got #{self.actual.to_display}"
    end
  end

  def not_to_eq(expected: T) -> ()
    if self.actual == expected
      Runner.case_mark_failed
      puts "FAILED: expected NOT #{expected.to_display}, got #{self.actual.to_display}"
    end
  end
```

(Requires `T: Displayable` — add it to the `where` clause.)

Update the run loop in `Runner.run_group` to reset + check per case:

```rx
      if case.pending
        pending = pending + 1
      else
        Runner.case_reset
        for hook in &before_chain
          hook.()
        end
        case.body.()
        for hook in &after_chain
          hook.()
        end
        if Runner.case_get_failed == 0
          passed = passed + 1
        else
          failed = failed + 1
        end
      end
```

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test tester_summary_counts_pass_fail_pending 2>&1 | tee tmp/test-cache/4.3-green.log
```

- [ ] **Step 6: Update earlier fixtures**

`test_tester_describe_it_eq.rx` and `test_tester_context_hooks.rx` from Tasks 4.1 / 4.2 reference `m.to_eq(...) == false` — now `to_eq` returns Unit. Rewrite both fixtures to use the new flag-druxen API:

In `test_tester_describe_it_eq.rx` simplify the `it` body to:

```rx
    t.it("adds two numbers") do
      t.expect(1 + 2).to_eq(3)
    end
```

And re-run the Phase 4.1 / 4.2 tests:

```bash
cargo test -p ruxen_core --test stdlib_test 2>&1 | tee tmp/test-cache/4.3-regress.log
```

Expected: all three Phase 4 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add library/std/test/runtime/test.c \
        library/std/test/src/runner.rx \
        library/std/test/src/matcher.rx \
        library/std/test/src/tester.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_tester_describe_it_eq.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_tester_context_hooks.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_tester_xit_and_fail.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): pass/fail/pending counts via per-case flag"
```

---

## Phase 5 — Fork-per-test isolation + expect_panic

Goal: a panic in one test doesn't kill its siblings. `expect_panic("substr") do ... end` works.

### Task 5.1: `ruxen_test_fork_and_wait` C shim

**Files:**
- Modify: `library/std/test/runtime/test.c`
- Test: write at next task (the shim alone has nothing to assert)

- [ ] **Step 1: Append the fork helper**

```c
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include <fcntl.h>

/* Fork; child invokes the Ruxen closure stored in `closure_handle`
 * (via the per-process indirect-call shim Ruxen uses for `f.()`); on
 * return, child writes the captured stderr to a parent-supplied pipe
 * and exits 0. Parent waitpid()s and returns a packed result:
 *
 *   bit 0:     1 if WIFEXITED + exit==0 (pass), else 0
 *   bits 1-8:  WEXITSTATUS if exited normally, else 0
 *   bits 9-16: WTERMSIG if killed by signal, else 0
 *   bits 17+:  reserved
 *
 * The stderr bytes are written to a file at `stderr_path` (parent-
 * provided string). Parent reads it to verify expect_panic substr.
 *
 * Limitation: we cannot directly call a Ruxen closure from C without
 * an indirect-call adapter Ruxen exposes. v1 implementation defers
 * the actual fork-then-invoke into Ruxen; this C entry only forks
 * and waits. See companion Ruxen-side `Runner.fork_each` (Task 5.2)
 * for the closure-invocation half.
 */

int64_t ruxen_test_fork(void) {
    pid_t pid = fork();
    if (pid < 0) {
        ruxen_panic("fork failed");
    }
    return (int64_t)pid;
}

int64_t ruxen_test_wait(int64_t pid) {
    int status = 0;
    pid_t result = waitpid((pid_t)pid, &status, 0);
    if (result < 0) {
        return -1;
    }
    int64_t packed = 0;
    if (WIFEXITED(status)) {
        if (WEXITSTATUS(status) == 0) {
            packed |= 1;
        }
        packed |= ((int64_t)WEXITSTATUS(status) & 0xff) << 1;
    }
    if (WIFSIGNALED(status)) {
        packed |= ((int64_t)WTERMSIG(status) & 0xff) << 9;
    }
    return packed;
}

/* Child-side: redirect stderr to a file so the parent can scan for
 * expect_panic substrings after waitpid returns. */
int64_t ruxen_test_redirect_stderr(int64_t path_handle, int64_t path_len) {
    const char *path = (const char *)path_handle;
    (void)path_len;
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) return -1;
    if (dup2(fd, 2) < 0) {
        close(fd);
        return -1;
    }
    close(fd);
    return 0;
}

/* Child-side: explicit exit. Used after the test body returns so we
 * don't fall through to the parent's continuing run_group loop. */
void ruxen_test_child_exit(int64_t code) {
    _exit((int)code);
}
```

- [ ] **Step 2: Expose them as lib decls on Runner**

Append to the `lib "runtime/test.c"` block in `library/std/test/src/runner.rx`:

```rx
    def self.fork as "ruxen_test_fork"() -> Int
    def self.wait as "ruxen_test_wait"(pid: Int) -> Int
    def self.redirect_stderr as "ruxen_test_redirect_stderr"(path: Int, len: Int) -> Int
    def self.child_exit as "ruxen_test_child_exit"(code: Int) -> ()
```

- [ ] **Step 3: Run the existing stdlib_test suite — expect still-green**

```bash
cargo test -p ruxen_core --test stdlib_test 2>&1 | tee tmp/test-cache/5.1-regress.log
```

Expected: all prior PASS, no regression (we've only added unused entry points).

- [ ] **Step 4: Commit**

```bash
git add library/std/test/runtime/test.c \
        library/std/test/src/runner.rx
git commit -m "stdlib(test): fork/wait/redirect-stderr C shims for test isolation"
```

### Task 5.2: Run each test case in a forked child

**Files:**
- Modify: `library/std/test/src/runner.rx` (`run_group` calls fork instead of in-process)
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_runner_fork_isolates_panic.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_runner_fork_isolates_panic.rx`:

```rx
use std.test.Tester
use std.test.Runner

def main
  let r = Runner.new("fixture.fork_isolation")
  Runner.box_store(r.handle_addr)

  Tester.describe("Survives panic") do |t|
    t.it("first - passes") do
      t.expect(1).to_eq(1)
    end
    t.it("second - panics") do
      panic!("simulated explosion")
    end
    t.it("third - still runs") do
      t.expect(2).to_eq(2)
    end
  end

  r.execute
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn runner_fork_isolates_panic() {
    let (stdout, _stderr, _ok) = compile_and_run(
        &rx("test_runner_fork_isolates_panic"),
        "stdlib_test_runner_fork_isolation",
    );
    assert!(stdout.contains("2 passed, 1 failed, 0 pending"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run — expect failure** (in-process panic kills the whole binary; only 1 pass observed before abort)

```bash
cargo test -p ruxen_core --test stdlib_test runner_fork_isolates_panic 2>&1 | tee tmp/test-cache/5.2-red.log
```

- [ ] **Step 4: Replace the in-process body invocation with a fork**

In `Runner.run_group`, replace the case-body block:

```rx
      if case.pending
        pending = pending + 1
      else
        let pid = Runner.fork
        if pid == 0
          # Child: run body, exit 0 on success, non-zero on failure.
          Runner.case_reset
          for hook in &before_chain
            hook.()
          end
          case.body.()
          for hook in &after_chain
            hook.()
          end
          let code = if Runner.case_get_failed == 0
            0
          else
            1
          end
          Runner.child_exit(code)
        else
          let packed = Runner.wait(pid)
          let pass_bit = packed & 1
          if pass_bit == 1
            passed = passed + 1
          else
            failed = failed + 1
          end
        end
      end
```

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test runner_fork_isolates_panic 2>&1 | tee tmp/test-cache/5.2-green.log
```

Also re-run the Phase 4 tests to verify no regression:

```bash
cargo test -p ruxen_core --test stdlib_test 2>&1 | tee tmp/test-cache/5.2-regress.log
```

Expected: all earlier tests still PASS.

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/runner.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_runner_fork_isolates_panic.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): fork-per-test isolation in Runner.run_group"
```

### Task 5.3: `expect_panic("substr") do ... end`

**Files:**
- Modify: `library/std/test/src/tester.rx` (add `expect_panic`)
- Modify: `library/std/test/src/test_case.rx` (set `expect_panic_substr` on the case)
- Modify: `library/std/test/src/runner.rx` (verify exit was nonzero + stderr contains substr)
- Test: `compiler/ruxen_core/tests/fixtures/ruxen/test_expect_panic.rx`
- Test: `compiler/ruxen_core/tests/stdlib_test.rs` (append)

- [ ] **Step 1: Write the failing fixture**

`compiler/ruxen_core/tests/fixtures/ruxen/test_expect_panic.rx`:

```rx
use std.test.Tester
use std.test.Runner

def main
  let r = Runner.new("fixture.expect_panic")
  Runner.box_store(r.handle_addr)

  Tester.describe("Panic-expecting tests") do |t|
    t.it("panics with matching substring -> pass") do
      t.expect_panic("kaboom") do
        panic!("kaboom went the dynamite")
      end
    end

    t.it("panics with non-matching substring -> fail") do
      t.expect_panic("wrong-word") do
        panic!("kaboom went the dynamite")
      end
    end

    t.it("does NOT panic -> fail") do
      t.expect_panic("never-thrown") do
        puts "no panic here"
      end
    end
  end

  r.execute
end
```

- [ ] **Step 2: Append failing Rust test**

```rust
#[test]
fn expect_panic_substring_match_drives_pass_fail() {
    let (stdout, _stderr, _ok) =
        compile_and_run(&rx("test_expect_panic"), "stdlib_test_expect_panic");
    assert!(stdout.contains("1 passed, 2 failed, 0 pending"), "got: {}", stdout);
}
```

- [ ] **Step 3: Run — expect failure** (no `expect_panic` method yet)

```bash
cargo test -p ruxen_core --test stdlib_test expect_panic_substring_match_drives_pass_fail 2>&1 | tee tmp/test-cache/5.3-red.log
```

- [ ] **Step 4: Wire `expect_panic` through the case + runner**

In `test_case.rx`, no schema change needed (we already have `expect_panic_substr: String?`).

In `tester.rx`, change `it` to accept an optional kind and add `expect_panic` as a body-scoped helper. The cleanest path:

```rx
  ## Inside an `it` body, schedule an inner expectation that the
  ## enclosed block panics. Implementation: temporarily store the
  ## expected substring in a process-static slot; the Runner sees the
  ## slot after the case body returns and reinterprets pass/fail.
  def expect_panic(substr: &String, body: any Fn() -> ()) -> ()
    Runner.set_expect_panic_substr(substr)
    body.()
    # If we got here without panicking, mark the case as failed.
    Runner.case_mark_failed
    puts "FAILED: expected panic containing '#{substr}', no panic occurred"
  end
```

Add to `runtime/test.c`:

```c
static char ruxen_test_expect_panic_buf[256] = {0};
static int ruxen_test_expect_panic_set = 0;

int64_t ruxen_test_set_expect_panic(int64_t str_ptr, int64_t str_len) {
    const char *s = (const char *)str_ptr;
    int n = (int)str_len;
    if (n >= (int)sizeof(ruxen_test_expect_panic_buf) - 1) {
        n = (int)sizeof(ruxen_test_expect_panic_buf) - 1;
    }
    if (n < 0) n = 0;
    memcpy(ruxen_test_expect_panic_buf, s, n);
    ruxen_test_expect_panic_buf[n] = 0;
    ruxen_test_expect_panic_set = 1;
    return 0;
}

int64_t ruxen_test_get_expect_panic_set(void) {
    return ruxen_test_expect_panic_set;
}

int64_t ruxen_test_get_expect_panic_substr(void) {
    return (int64_t)ruxen_test_expect_panic_buf;
}

int64_t ruxen_test_clear_expect_panic(void) {
    ruxen_test_expect_panic_set = 0;
    ruxen_test_expect_panic_buf[0] = 0;
    return 0;
}
```

Add lib decls on Runner:

```rx
    def self.set_expect_panic_substr as "ruxen_test_set_expect_panic"(s: Int, n: Int) -> Int
    def self.get_expect_panic_set as "ruxen_test_get_expect_panic_set"() -> Int
    def self.clear_expect_panic as "ruxen_test_clear_expect_panic"() -> Int
```

In `Runner.run_group`, after the case body returns in the child, decide:

```rx
          case.body.()
          for hook in &after_chain
            hook.()
          end
          let code = if Runner.get_expect_panic_set == 1
            # expect_panic was set, no panic happened — fail.
            Runner.clear_expect_panic
            2
          elsif Runner.case_get_failed == 0
            0
          else
            1
          end
          Runner.child_exit(code)
```

In the parent's wait branch:

```rx
          let packed = Runner.wait(pid)
          let pass_bit = packed & 1
          let exit_code = (packed >> 1) & 255
          let signaled = (packed >> 9) & 255
          if signaled != 0
            # Child died on signal (abort from panic!).
            # If this case set expect_panic_substr at the t.expect_panic
            # call site, the substring check happens here against the
            # child's stderr — for v1 we accept any panic as a match
            # because substring verification needs a parent-side stderr
            # capture path that's deferred to Phase 6.
            # TODO(post-v1): real substring match.
            # Treat panic-when-expected as pass; panic-when-not-expected
            # as fail.
            if case.expect_panic_substr != nil
              passed = passed + 1
            else
              failed = failed + 1
            end
          elsif pass_bit == 1
            passed = passed + 1
          else
            failed = failed + 1
          end
```

For the v1 substring match to work, the **`Tester#expect_panic` MUST record the substring onto the TestCase itself** (so the parent has access — module-static state is in the child after fork). Restructure: make `expect_panic` a Tester method that, instead of taking a closure inline, RESHAPES the surrounding `it` definition. That doesn't work cleanly without macros.

**Pragmatic v1 cut:** `expect_panic` accepts the substring; the substring is recorded onto the **last registered TestCase in the current group** at DSL time, not at body run time. This means `expect_panic` must be called at the top of the `it` block, before any other expectations:

```rx
  def expect_panic(substr: &String, body: any Fn() -> ()) -> ()
    let last_idx = self.cases.len - 1
    if last_idx >= 0
      let case_ref = &var self.cases[last_idx]
      case_ref.expect_panic_substr = substr.clone
    end
    body.()
  end
```

But this Tester method is called INSIDE an `it` body — `self` here is the inner Tester for that case, not the parent group. The proper plumbing: pass the active `case_ref` into the it-body closure as an implicit. Too invasive for v1.

**Simplest v1 acceptable cut:** `expect_panic("substr") do ... end` ALWAYS passes if the body panics with ANY message, ALWAYS fails if it doesn't panic. The substring is ignored in v1 (recorded but not verified). Document in the spec under "Known v1 limitations".

Replace the `expect_panic` body with:

```rx
  ## v1: substring is accepted for forward compatibility but NOT verified.
  ## A child panic always satisfies expect_panic; an absent panic always
  ## fails it. Per-message substring verification ships in v1.1 once the
  ## parent-side stderr capture path lands.
  def expect_panic(substr: &String, body: any Fn() -> ()) -> ()
    Runner.set_expect_panic_substr(substr)
    body.()
    Runner.case_mark_failed
  end
```

And in `Runner.run_group` child:

```rx
          let code = if Runner.get_expect_panic_set == 1
            Runner.clear_expect_panic
            # Body returned but expect_panic was set → no panic → fail.
            2
          elsif Runner.case_get_failed == 0
            0
          else
            1
          end
```

Parent:

```rx
          if signaled != 0 || exit_code != 0
            # Child died abnormally (panic or fail). If expect_panic was
            # set in the child, this is exactly what we wanted — but
            # the parent can't see the child's `set` flag (different
            # process). v1 cut: encode expect_panic_was_set in the
            # exit code: 0=pass, 1=normal-fail, 2=expect_panic_not_thrown.
            # signal=any => panic; pass iff the child intended expect_panic.
            #
            # The case body itself must signal intent before forking.
            # In v1 we sniff for the literal substring presence by
            # asking the in-process Tester whether THIS case scheduled
            # an expect_panic earlier — but that flag was set IN THE
            # CHILD. Workaround: the parent inspects the case's
            # `expect_panic_substr` Ruxen field, which was set
            # synchronously in the it-body BEFORE fork in v1.
            ...
```

This is getting complicated. **Resolve the architecture cleanly:** record `expect_panic_substr` on the TestCase **at DSL setup time** — the user writes `t.expect_panic("substr")` at the top of the `it` BLOCK BODY but in v1 we move it to be a DSL-setup-time call:

```rx
    t.it("panics on overflow") do
      t.expect_panic_substr = "overflow"   # decorator-style
      compute_overflow()
    end
```

OR use an alternate `it` overload:

```rx
    t.it_panics("never-thrown", "expected-substr") do
      compute_no_panic()
    end
```

For v1, ship **`it_panics(name, substr) do ... end`** instead of inline `expect_panic`. The DSL is uglier but the implementation is clean:

In `tester.rx`:

```rx
  def it_panics(name: &String, expected_substr: &String,
                body: any Fn() -> ()) -> ()
    var tc = TestCase.new(name, body)
    tc.expect_panic_substr = expected_substr.clone
    self.cases.push(tc)
  end
```

In `Runner.run_group` parent branch:

```rx
          let expects_panic = case.expect_panic_substr != nil
          if expects_panic
            # Pass iff child died on signal OR exited nonzero.
            # (v1 substring NOT verified — see Known limitations.)
            if signaled != 0 || exit_code != 0
              passed = passed + 1
            else
              failed = failed + 1
            end
          else
            if pass_bit == 1
              passed = passed + 1
            else
              failed = failed + 1
            end
          end
```

Rewrite the fixture for the simpler API:

`test_expect_panic.rx`:

```rx
use std.test.Tester
use std.test.Runner

def main
  let r = Runner.new("fixture.expect_panic")
  Runner.box_store(r.handle_addr)

  Tester.describe("Panic-expecting tests") do |t|
    t.it_panics("panics -> pass", "kaboom") do
      panic!("kaboom went the dynamite")
    end

    t.it_panics("does NOT panic -> fail", "never-thrown") do
      puts "no panic here"
    end
  end

  r.execute
end
```

Update the Rust expectation:

```rust
#[test]
fn it_panics_pass_when_body_panics_fail_when_not() {
    let (stdout, _stderr, _ok) =
        compile_and_run(&rx("test_expect_panic"), "stdlib_test_expect_panic");
    assert!(stdout.contains("1 passed, 1 failed, 0 pending"), "got: {}", stdout);
}
```

- [ ] **Step 5: Run — expect pass**

```bash
cargo test -p ruxen_core --test stdlib_test it_panics_pass_when_body_panics_fail_when_not 2>&1 | tee tmp/test-cache/5.3-green.log
```

- [ ] **Step 6: Commit**

```bash
git add library/std/test/src/tester.rx \
        library/std/test/src/runner.rx \
        compiler/ruxen_core/tests/fixtures/ruxen/test_expect_panic.rx \
        compiler/ruxen_core/tests/stdlib_test.rs
git commit -m "stdlib(test): it_panics(name, substr) — fork-detected panic expectation (v1 substring not yet verified)"
```

**Open follow-up** (NOT v1): proper substring verification needs the parent to read the child's stderr. Add later by routing the child's stderr to a per-PID file via `ruxen_test_redirect_stderr` (already in shim from Task 5.1) and grepping it after `wait` returns. Track this in the spec's "Known limitations" section.

---

## Phase 6 — `ruxen test` CLI subcommand

Goal: end-to-end CLI flow — `ruxen test` in a project with `tests/foo.rx` discovers, wraps, builds, runs sequentially. Parallelism + formats land in subsequent tasks.

### Task 6.1: Replace the CLI placeholder with real argument shape

**Files:**
- Modify: `src/ruxen_cli/src/cli.rs:174-179`

- [ ] **Step 1: Expand the `Test` variant**

```rust
/// Test framework — discover `tests/**.rx`, build per-file binaries
/// via the incremental cache, fork-per-test for isolation.
Test {
    /// Substring filter on test names (positional)
    filter: Option<String>,
    /// Build tests in release mode
    #[arg(long)]
    release: bool,
    /// Parallel fan-out width; "auto" = min(ncpus, 8)
    #[arg(long = "test-threads", default_value = "auto")]
    test_threads: String,
    /// Stop dispatching after first failure
    #[arg(long = "fail-fast")]
    fail_fast: bool,
    /// Don't capture test stdout/stderr
    #[arg(long)]
    nocapture: bool,
    /// List discovered tests; don't run
    #[arg(long)]
    list: bool,
    /// Build but don't execute
    #[arg(long = "no-run")]
    no_run: bool,
    /// Include xit (pending) tests in execution
    #[arg(long = "include-pending")]
    include_pending: bool,
    /// Output format: pretty | tap | json
    #[arg(long, default_value = "pretty")]
    format: String,
},
```

- [ ] **Step 2: Verify the workspace still builds**

```bash
cargo build -p ruxen_cli 2>&1 | tee tmp/test-cache/6.1-build.log
```

Expected: succeeds; `cli::Command::Test { filter: _ }` match arm in `main.rs` is now non-exhaustive — will fail next step.

- [ ] **Step 3: Update the match arm in main.rs to use the new fields**

In `src/ruxen_cli/src/main.rs:116-122`:

```rust
cli::Command::Test {
    filter,
    release,
    test_threads,
    fail_fast,
    nocapture,
    list,
    no_run,
    include_pending,
    format,
} => ruxenc::test_runner::run(ruxenc::test_runner::TestOptions {
    filter,
    release,
    test_threads,
    fail_fast,
    nocapture,
    list,
    no_run,
    include_pending,
    format,
}),
```

- [ ] **Step 4: Verify build fails at the missing module — this is expected**

```bash
cargo build -p ruxen_cli 2>&1 | tee tmp/test-cache/6.1-build-fail.log
```

Expected: error "could not find `test_runner` in `ruxenc`". Next task builds the module.

- [ ] **Step 5: Commit the CLI surface (compile-broken intentionally)**

Actually, don't commit yet — broken build. Defer the commit until Task 6.2 lands the module. Re-stage these files at end of 6.2.

### Task 6.2: `test_runner.rs` skeleton + discovery

**Files:**
- Create: `src/ruxenc/src/test_runner.rs`
- Modify: `src/ruxenc/src/lib.rs` (add `pub mod test_runner;`)

- [ ] **Step 1: Write the module skeleton**

`src/ruxenc/src/test_runner.rs`:

```rust
//! `ruxen test` — discover `tests/**.rx`, wrap each in a synthesised
//! `def main`, build per-file binaries through the incremental cache,
//! and dispatch them in parallel with fork-per-test isolation
//! supplied by `library/std/test/runtime/test.c`.
//!
//! See docs/superpowers/specs/2026-05-23-test-framework-design.md.

use std::fs;
use std::path::{Path, PathBuf};

pub struct TestOptions {
    pub filter: Option<String>,
    pub release: bool,
    pub test_threads: String,
    pub fail_fast: bool,
    pub nocapture: bool,
    pub list: bool,
    pub no_run: bool,
    pub include_pending: bool,
    pub format: String,
}

pub fn run(opts: TestOptions) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let files = discover_test_files(&project_dir)?;

    if files.is_empty() {
        println!("no test files found under tests/");
        return Ok(());
    }

    if opts.list {
        for f in &files {
            println!("{}", test_path_for(&project_dir, f));
        }
        return Ok(());
    }

    // TODO Phase 6.3: synthesise wrappers
    // TODO Phase 6.4: build via incremental cache
    // TODO Phase 6.5: execute and aggregate
    eprintln!("discovered {} test file(s); build+exec wired in 6.3+", files.len());
    Ok(())
}

/// Walk upward from CWD until we find a Ruxen.toml.
fn find_project_root() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir()
        .map_err(|e| format!("cannot read cwd: {}", e))?;
    loop {
        if dir.join("Ruxen.toml").exists() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return Err("no Ruxen.toml found in CWD or any ancestor".into()),
        }
    }
}

/// Collect every `.rx` file under `<project_dir>/tests/` EXCEPT those
/// under `tests/support/` (helper modules — see spec §4.3).
fn discover_test_files(project_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let tests_dir = project_dir.join("tests");
    if !tests_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    walk(&tests_dir, &tests_dir, &mut out);
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            // Exclude tests/support/ subtree.
            if dir == root && p.file_name() == Some(std::ffi::OsStr::new("support")) {
                continue;
            }
            walk(root, &p, out);
        } else if p.extension() == Some(std::ffi::OsStr::new("rx")) {
            out.push(p);
        }
    }
}

/// Convert `<project>/tests/foo/bar.rx` -> "foo.bar".
fn test_path_for(project_dir: &Path, file: &Path) -> String {
    let tests = project_dir.join("tests");
    let rel = file.strip_prefix(&tests).unwrap_or(file);
    let mut s = rel.with_extension("").to_string_lossy().into_owned();
    if std::path::MAIN_SEPARATOR != '.' {
        s = s.replace(std::path::MAIN_SEPARATOR, ".");
    }
    s
}
```

`src/ruxenc/src/lib.rs` — add `pub mod test_runner;`.

- [ ] **Step 2: Verify the workspace builds**

```bash
cargo build -p ruxen_cli 2>&1 | tee tmp/test-cache/6.2-build.log
```

Expected: PASS.

- [ ] **Step 3: Smoke test discovery**

```bash
mkdir -p /tmp/ruxen-test-smoke/tests/calculator
mkdir -p /tmp/ruxen-test-smoke/tests/support
cat > /tmp/ruxen-test-smoke/Ruxen.toml <<'EOF'
[package]
name = "smoke"
version = "0.0.1"
EOF
touch /tmp/ruxen-test-smoke/tests/calculator/math.rx
touch /tmp/ruxen-test-smoke/tests/calculator/edge.rx
touch /tmp/ruxen-test-smoke/tests/support/factories.rx
( cd /tmp/ruxen-test-smoke && cargo run -q -p ruxen_cli -- test --list ) 2>&1 | tee tmp/test-cache/6.2-list.log
```

Expected: two lines printed (`calculator.edge`, `calculator.math`), NOT `support.factories`.

- [ ] **Step 4: Commit Phase 6.1 + 6.2 together**

```bash
git add src/ruxen_cli/src/cli.rs \
        src/ruxen_cli/src/main.rs \
        src/ruxenc/src/lib.rs \
        src/ruxenc/src/test_runner.rs
git commit -m "cli(test): TestOptions + discovery (tests/**.rx, excludes support/)"
```

### Task 6.3: Synthesise wrapper file

**Files:**
- Modify: `src/ruxenc/src/test_runner.rs` (add `synthesise_wrapper`)

- [ ] **Step 1: Write the synthesiser**

Add to `test_runner.rs`:

```rust
/// Generate the wrapper .rx file that compiles to the per-file
/// test binary. Layout (textual concatenation):
///
///   <prelude>
///   <user file body verbatim>
///   <postlude>
///
/// The user's file may contain ONLY expression-statements at top level
/// (Tester.describe calls, lets, ...). Top-level def/class/use AFTER
/// the prelude will be syntactically illegal — fine for v1 since helper
/// code goes in tests/support/.
fn synthesise_wrapper(
    test_path: &str,
    user_file: &Path,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    let body = fs::read_to_string(user_file)
        .map_err(|e| format!("read {}: {}", user_file.display(), e))?;

    let prelude = format!(
        "# AUTO-GENERATED from {} — do not edit.\n\
         use std.test.Tester\n\
         use std.test.Runner\n\
         \n\
         def main\n  \
           let r = Runner.new(\"{}\")\n  \
           Runner.box_store(r.handle_addr)\n",
        user_file.display(),
        test_path.replace('"', "\\\""),
    );
    let postlude = "\n  r.execute\nend\n";

    let synth = format!("{prelude}{body}\n{postlude}");

    fs::create_dir_all(out_dir)
        .map_err(|e| format!("mkdir {}: {}", out_dir.display(), e))?;
    let synth_path = out_dir.join(format!("{}.synth.rx", test_path.replace('.', "_")));
    fs::write(&synth_path, &synth)
        .map_err(|e| format!("write {}: {}", synth_path.display(), e))?;
    Ok(synth_path)
}
```

- [ ] **Step 2: Add a unit test for the synthesiser**

Append to `test_runner.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn synthesise_wraps_user_body_with_runner() {
        let tmp = std::env::temp_dir().join("test-runner-synth-1");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let user_file = tmp.join("user.rx");
        fs::write(&user_file, "Tester.describe(\"X\") do |t|\n  t.it(\"y\") do\n    t.expect(1).to_eq(1)\n  end\nend").unwrap();
        let synth_path = synthesise_wrapper("foo.bar", &user_file, &tmp.join("out")).unwrap();
        let synth = fs::read_to_string(&synth_path).unwrap();
        assert!(synth.contains("def main"), "synth: {synth}");
        assert!(synth.contains("Runner.new(\"foo.bar\")"), "synth: {synth}");
        assert!(synth.contains("Tester.describe(\"X\")"), "synth: {synth}");
        assert!(synth.contains("r.execute"), "synth: {synth}");
        assert!(synth.contains("end\n"), "synth: {synth}");
    }
}
```

- [ ] **Step 3: Run — expect pass**

```bash
cargo test -p ruxenc test_runner::tests::synthesise_wraps_user_body_with_runner 2>&1 | tee tmp/test-cache/6.3-green.log
```

- [ ] **Step 4: Commit**

```bash
git add src/ruxenc/src/test_runner.rs
git commit -m "cli(test): wrapper synthesiser — prelude + body + postlude"
```

### Task 6.4: Wire build + sequential execution

**Files:**
- Modify: `src/ruxenc/src/test_runner.rs` (call `compile::run` + spawn binary)

- [ ] **Step 1: Add the build+exec helper**

Append to `test_runner.rs`:

```rust
use std::process::{Command, Stdio};

fn build_and_run(
    project_dir: &Path,
    test_path: &str,
    user_file: &Path,
    out_dir: &Path,
    release: bool,
    no_run: bool,
) -> Result<TestFileResult, String> {
    let synth = synthesise_wrapper(test_path, user_file, out_dir)?;
    let profile = if release { "release" } else { "debug" };
    let bin_dir = project_dir.join("target").join(profile).join("test");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    let bin_path = bin_dir.join(test_path.replace('.', "_"));

    let mut compile_args = vec![
        "ruxenc".to_string(),
        synth.to_string_lossy().into_owned(),
        "-o".to_string(),
        bin_path.to_string_lossy().into_owned(),
    ];
    if release {
        compile_args.push("--release".to_string());
    }
    crate::compile::run(&compile_args)
        .map_err(|e| format!("compile of {test_path}: {e}"))?;

    if no_run {
        return Ok(TestFileResult {
            test_path: test_path.to_string(),
            passed: 0,
            failed: 0,
            pending: 0,
            stdout: String::new(),
            stderr: String::new(),
            exit_ok: true,
        });
    }

    let output = Command::new(&bin_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("spawn {}: {}", bin_path.display(), e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let (passed, failed, pending) = parse_summary_line(&stdout);

    Ok(TestFileResult {
        test_path: test_path.to_string(),
        passed,
        failed,
        pending,
        stdout,
        stderr,
        exit_ok: output.status.success(),
    })
}

pub struct TestFileResult {
    pub test_path: String,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    pub stdout: String,
    pub stderr: String,
    pub exit_ok: bool,
}

/// Parse the Runner.execute summary line "N passed, M failed, K pending".
fn parse_summary_line(stdout: &str) -> (u32, u32, u32) {
    for line in stdout.lines().rev() {
        let s = line.trim();
        if !s.contains("passed") || !s.contains("failed") || !s.contains("pending") {
            continue;
        }
        let parts: Vec<&str> = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).collect();
        if parts.len() >= 3 {
            return (
                parts[0].parse().unwrap_or(0),
                parts[1].parse().unwrap_or(0),
                parts[2].parse().unwrap_or(0),
            );
        }
    }
    (0, 0, 0)
}
```

Wire it into `run`:

```rust
pub fn run(opts: TestOptions) -> Result<(), String> {
    let project_dir = find_project_root()?;
    let files = discover_test_files(&project_dir)?;

    if files.is_empty() {
        println!("no test files found under tests/");
        return Ok(());
    }

    if opts.list {
        for f in &files {
            println!("{}", test_path_for(&project_dir, f));
        }
        return Ok(());
    }

    let out_dir = project_dir.join("target").join("ruxen").join("test-build");
    let mut total_passed = 0u32;
    let mut total_failed = 0u32;
    let mut total_pending = 0u32;

    for f in &files {
        let tp = test_path_for(&project_dir, f);
        let result = build_and_run(&project_dir, &tp, f, &out_dir, opts.release, opts.no_run)?;
        if !opts.nocapture && !result.stdout.is_empty() && result.failed == 0 {
            // suppress stdout on green
        } else {
            print!("{}", result.stdout);
            eprint!("{}", result.stderr);
        }
        total_passed += result.passed;
        total_failed += result.failed;
        total_pending += result.pending;

        if opts.fail_fast && result.failed > 0 {
            break;
        }
    }

    println!("\ntest result: {}. {} passed; {} failed; {} pending",
        if total_failed == 0 { "ok" } else { "FAILED" },
        total_passed, total_failed, total_pending);

    if total_failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
```

- [ ] **Step 2: Build**

```bash
cargo build -p ruxen_cli 2>&1 | tee tmp/test-cache/6.4-build.log
```

- [ ] **Step 3: End-to-end smoke against a real Ruxen project**

```bash
rm -rf /tmp/ruxen-test-e2e
mkdir -p /tmp/ruxen-test-e2e/tests
cat > /tmp/ruxen-test-e2e/Ruxen.toml <<'EOF'
[package]
name = "e2e_smoke"
version = "0.0.1"
EOF
cat > /tmp/ruxen-test-e2e/tests/example.rx <<'EOF'
Tester.describe("e2e smoke") do |t|
  t.it("adds") do
    t.expect(1 + 1).to_eq(2)
  end
  t.it("fails on purpose") do
    t.expect(1 + 1).to_eq(3)
  end
end
EOF
( cd /tmp/ruxen-test-e2e && cargo run -q -p ruxen_cli -- test ) 2>&1 | tee tmp/test-cache/6.4-e2e.log
echo "---exit=$?---" | tee -a tmp/test-cache/6.4-e2e.log
```

Expected: stdout ends with `1 passed; 1 failed; 0 pending`, exit code 1.

- [ ] **Step 4: Commit**

```bash
git add src/ruxenc/src/test_runner.rs
git commit -m "cli(test): sequential build + run + summary aggregation"
```

### Task 6.5: Parallel dispatch (per-file)

**Files:**
- Modify: `src/ruxenc/src/test_runner.rs` (replace serial loop with rayon or thread::spawn pool)
- Modify: `src/ruxenc/Cargo.toml` (add `rayon = "1"` if not already present; otherwise hand-rolled threads)

- [ ] **Step 1: Decide parallelism source**

Check if `rayon` is already a workspace dep:

```bash
grep -rn "rayon" src/ruxenc/Cargo.toml Cargo.toml 2>/dev/null
```

If present, use `rayon::scope`. If absent, use `std::thread::scope` (no new dep — preferred for a tiny pool).

- [ ] **Step 2: Implement bounded-concurrency thread pool**

In `test_runner.rs`, replace the serial `for f in &files` loop:

```rust
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

fn resolve_test_threads(s: &str) -> usize {
    if s == "auto" {
        std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(1)
    } else {
        s.parse::<usize>().ok().filter(|&n| n > 0).unwrap_or(1)
    }
}

// ... inside run():
let n_workers = resolve_test_threads(&opts.test_threads);
let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
let results: Arc<Mutex<Vec<TestFileResult>>> = Arc::new(Mutex::new(Vec::new()));

let queue: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(files.clone()));

std::thread::scope(|scope| {
    for _ in 0..n_workers {
        let queue = queue.clone();
        let results = results.clone();
        let stop = stop.clone();
        let out_dir = out_dir.clone();
        let project_dir = project_dir.clone();
        let release = opts.release;
        let no_run = opts.no_run;
        let fail_fast = opts.fail_fast;
        scope.spawn(move || {
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let f = {
                    let mut q = queue.lock().unwrap();
                    if q.is_empty() { return; }
                    q.remove(0)
                };
                let tp = test_path_for(&project_dir, &f);
                let result = build_and_run(&project_dir, &tp, &f, &out_dir, release, no_run);
                let r = match result {
                    Ok(r) => r,
                    Err(e) => TestFileResult {
                        test_path: tp,
                        passed: 0, failed: 1, pending: 0,
                        stdout: String::new(),
                        stderr: format!("build error: {}", e),
                        exit_ok: false,
                    }
                };
                let had_failure = r.failed > 0 || !r.exit_ok;
                results.lock().unwrap().push(r);
                if fail_fast && had_failure {
                    stop.store(true, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            }
        });
    }
});

let results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
let mut total_passed = 0u32; let mut total_failed = 0u32; let mut total_pending = 0u32;
for r in &results {
    if !opts.nocapture && r.failed == 0 && r.exit_ok {
        // suppress
    } else {
        print!("--- {} ---\n{}", r.test_path, r.stdout);
        eprint!("{}", r.stderr);
    }
    total_passed += r.passed; total_failed += r.failed; total_pending += r.pending;
}

println!("\ntest result: {}. {} passed; {} failed; {} pending",
    if total_failed == 0 { "ok" } else { "FAILED" },
    total_passed, total_failed, total_pending);

if total_failed > 0 { std::process::exit(1); }
Ok(())
```

- [ ] **Step 3: Smoke test with 4 files**

```bash
rm -rf /tmp/ruxen-test-parallel
mkdir -p /tmp/ruxen-test-parallel/tests
cat > /tmp/ruxen-test-parallel/Ruxen.toml <<'EOF'
[package]
name = "parallel_smoke"
version = "0.0.1"
EOF
for n in 1 2 3 4; do
  cat > /tmp/ruxen-test-parallel/tests/case_${n}.rx <<EOF
Tester.describe("case ${n}") do |t|
  t.it("passes") do
    t.expect(${n}).to_eq(${n})
  end
end
EOF
done
( cd /tmp/ruxen-test-parallel && cargo run -q -p ruxen_cli -- test --test-threads=4 ) 2>&1 | tee tmp/test-cache/6.5-parallel.log
```

Expected: `4 passed; 0 failed; 0 pending`, all four files reported.

- [ ] **Step 4: Commit**

```bash
git add src/ruxenc/src/test_runner.rs
git commit -m "cli(test): bounded thread-pool parallel dispatch (--test-threads)"
```

---

## Phase 7 — Output formats (TAP + JSON) and polish

### Task 7.1: TAP output

**Files:**
- Create: `src/ruxenc/src/test_output.rs`
- Modify: `src/ruxenc/src/lib.rs` (`pub mod test_output;`)
- Modify: `src/ruxenc/src/test_runner.rs` (dispatch on `opts.format`)

- [ ] **Step 1: Implement TAP renderer**

`src/ruxenc/src/test_output.rs`:

```rust
//! Output renderers for `ruxen test`: pretty (default), tap, json.

use crate::test_runner::TestFileResult;

pub fn render_tap(results: &[TestFileResult]) {
    let total: u32 = results.iter().map(|r| r.passed + r.failed + r.pending).sum();
    println!("TAP version 13");
    println!("1..{}", total);

    let mut n = 0u32;
    for r in results {
        for _ in 0..r.passed {
            n += 1;
            println!("ok {} - {}", n, r.test_path);
        }
        for _ in 0..r.failed {
            n += 1;
            println!("not ok {} - {}", n, r.test_path);
            println!("  ---");
            println!("  message: \"see captured stderr for {}\"", r.test_path);
            println!("  ...");
        }
        for _ in 0..r.pending {
            n += 1;
            println!("ok {} - {} # SKIP pending", n, r.test_path);
        }
    }
}

pub fn render_json(results: &[TestFileResult]) {
    for r in results {
        for _ in 0..r.passed {
            println!("{{\"type\":\"test\",\"event\":\"ok\",\"name\":{:?}}}", r.test_path);
        }
        for _ in 0..r.failed {
            println!("{{\"type\":\"test\",\"event\":\"failed\",\"name\":{:?}}}", r.test_path);
        }
        for _ in 0..r.pending {
            println!("{{\"type\":\"test\",\"event\":\"ignored\",\"name\":{:?}}}", r.test_path);
        }
    }
    let total_passed: u32 = results.iter().map(|r| r.passed).sum();
    let total_failed: u32 = results.iter().map(|r| r.failed).sum();
    let total_pending: u32 = results.iter().map(|r| r.pending).sum();
    println!("{{\"type\":\"suite\",\"event\":{:?},\"passed\":{},\"failed\":{},\"ignored\":{}}}",
        if total_failed == 0 { "ok" } else { "failed" },
        total_passed, total_failed, total_pending);
}
```

In `test_runner.rs`, replace the per-result print + summary block:

```rust
match opts.format.as_str() {
    "tap" => crate::test_output::render_tap(&results),
    "json" => crate::test_output::render_json(&results),
    _ => {
        // pretty (existing path)
        for r in &results { /* ... existing ... */ }
        println!("\ntest result: {}. {} passed; {} failed; {} pending",
            if total_failed == 0 { "ok" } else { "FAILED" },
            total_passed, total_failed, total_pending);
    }
}
```

- [ ] **Step 2: Smoke test**

```bash
( cd /tmp/ruxen-test-parallel && cargo run -q -p ruxen_cli -- test --format=tap ) 2>&1 | tee tmp/test-cache/7.1-tap.log
( cd /tmp/ruxen-test-parallel && cargo run -q -p ruxen_cli -- test --format=json ) 2>&1 | tee tmp/test-cache/7.1-json.log
```

Expected: TAP-13 header + 4 `ok N - case_N` lines; JSON: one event per test + summary line.

- [ ] **Step 3: Commit**

```bash
git add src/ruxenc/src/test_output.rs \
        src/ruxenc/src/lib.rs \
        src/ruxenc/src/test_runner.rs
git commit -m "cli(test): TAP + JSON output formats"
```

### Task 7.2: `ruxen new` scaffolds an example test

**Files:**
- Modify: `src/ruxen_cli/src/scaffold.rs`

- [ ] **Step 1: Locate the scaffold function**

```bash
grep -n "fn new_project\|tests/" src/ruxen_cli/src/scaffold.rs
```

- [ ] **Step 2: Add a `tests/` directory creation + example.rx write**

In the `new_project` function, after the `src/main.rx` creation:

```rust
let tests_dir = root.join("tests");
fs::create_dir_all(&tests_dir)
    .map_err(|e| format!("create tests/: {}", e))?;
fs::write(
    tests_dir.join("example.rx"),
    "Tester.describe(\"example\") do |t|\n  \
       t.it(\"adds two numbers\") do\n    \
         t.expect(1 + 1).to_eq(2)\n  \
       end\nend\n",
).map_err(|e| format!("write tests/example.rx: {}", e))?;
```

- [ ] **Step 3: Smoke test scaffold**

```bash
rm -rf /tmp/ruxen-new-test && cargo run -q -p ruxen_cli -- new --no-git /tmp/ruxen-new-test 2>&1 | tee tmp/test-cache/7.2-scaffold.log
ls /tmp/ruxen-new-test/tests/
( cd /tmp/ruxen-new-test && cargo run -q -p ruxen_cli -- test ) 2>&1 | tee tmp/test-cache/7.2-runtest.log
```

Expected: `tests/example.rx` exists; `ruxen test` reports `1 passed; 0 failed; 0 pending`.

- [ ] **Step 4: Commit**

```bash
git add src/ruxen_cli/src/scaffold.rs
git commit -m "cli(new): scaffold tests/example.rx"
```

### Task 7.3: Tutorial documentation

**Files:**
- Create: `docs/tutorial/17-testing.md`

- [ ] **Step 1: Write the tutorial page**

`docs/tutorial/17-testing.md`:

```markdown
# 17. Testing

Ruxen ships a pure-Ruxen test framework. Tests live in `tests/**.rx`.
Each `.rx` file under `tests/` is a test file — no method-name
convention. Run them with `ruxen test`.

## A first test

```rx
Tester.describe("Calculator") do |t|
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

## Structure

- `Tester.describe(name) do |t| ... end` — opens a group. Always the
  first call in a test file.
- `t.context(name) do |t| ... end` — nested group. Inherits parent
  `before` / `after` hooks.
- `t.it(name) do ... end` — one test case.
- `t.xit(name) do ... end` — pending test (block not executed).
- `t.before do ... end` — run before each `it` in this group.
- `t.after do ... end` — run after each `it` in this group.

## Matchers

- `t.expect(actual).to_eq(expected)` — equality (requires `T: PartialEq`).
- `t.expect(actual).not_to_eq(expected)` — inequality.
- `BoolMatcher.new(actual).to_be_truthy` / `.to_be_falsy`.
- `OptionMatcher.new(opt.is_some).to_be_nil` / `.not_to_be_nil`.
- `ArrayMatcher.new(&xs).to_include(value)`.
- `StringMatcher.new(&s).to_include(needle)`.

## Expecting a panic

Use `it_panics(name, expected_substring) do ... end` instead of `it`:

```rx
t.it_panics("overflow", "overflow") do
  Int.max + 1
end
```

(v1 limitation: the expected substring is accepted but not verified;
any panic from the body satisfies the assertion.)

## Hooks and helpers

`before` / `after` run before / after each test in the surrounding
group. They inherit downward into nested `context` blocks.

Shared helpers (factories, fixtures) go in `tests/support/**.rx` —
those files are NOT executed as test files; you import them by name:

```rx
use tests.support.factories.{build_user}

Tester.describe("user") do |t|
  t.it("has a name") do
    let u = build_user
    t.expect(u.name).to_eq("Alice")
  end
end
```

## Command-line options

- `ruxen test FILTER` — substring filter on test path.
- `ruxen test --release` — build tests in release mode.
- `ruxen test --test-threads=N` — limit parallelism (default `min(ncpus, 8)`).
- `ruxen test --fail-fast` — stop after first failure.
- `ruxen test --nocapture` — pass through test stdout / stderr live.
- `ruxen test --list` — list discovered tests; don't run.
- `ruxen test --no-run` — build but don't execute.
- `ruxen test --include-pending` — execute `xit` blocks too.
- `ruxen test --format=pretty|tap|json` — output format.

## What's not in v1

- `let` / `subject` memoised helpers.
- `before(:all)` / `after(:all)` / `around`.
- Custom matchers, `change { }`, `to_satisfy`.
- Mocking / `double`.
- Shared examples.
- Property testing.
- `--timeout`.
- Per-test memory-leak reporting (tests leak today; the fork-per-test
  model means the OS reclaims, so suites still pass — but watch RSS
  on long suites until `Drop` lands fully).
```

- [ ] **Step 2: Commit**

```bash
git add docs/tutorial/17-testing.md
git commit -m "docs(tutorial): 17-testing.md — user guide for the test framework"
```

---

## Phase 8 — Final integration test

This is the **only** task that runs the full repo test suite. Per rule 42 — incremental tasks ran only their own narrow tests.

### Task 8.1: Full integration sweep

**Files:** none modified; this is verification.

- [ ] **Step 1: Run the full workspace test suite**

```bash
mkdir -p tmp/test-cache
cargo test --workspace 2>&1 | tee tmp/test-cache/8.1-full.log
```

Expected: PASS. If any failure surfaces, it is either:
(a) a regression caused by this branch — investigate.
(b) a pre-existing flake — diff against the pre-branch baseline (if a `tmp/test-cache/full-baseline.log` exists from before this work started).

- [ ] **Step 2: Verify the e2e release smoke didn't regress**

```bash
cargo test --test release_e2e_smoke -- --ignored --list 2>&1 | head -5
RUXEN_E2E_CASES=01_hello cargo test --test release_e2e_smoke -- --ignored 2>&1 | tee tmp/test-cache/8.1-e2e-hello.log
```

Expected: 01_hello passes (smoke that the toolchain isn't broken).

- [ ] **Step 3: Run the bench harness once to verify no co-location regression**

```bash
cargo run -q --bin ruxenc -- bench library/std/bench/src/lib.rx 2>&1 | head -5
```

(May not produce output if no `bench_*` fns in lib.rx — that's fine; the goal is to verify ruxenc still builds and dispatches.)

- [ ] **Step 4: Commit a sentinel marker if any follow-up tasks emerged**

If any failure surfaced and was fixed in this task, commit the fix. Otherwise skip the commit.

---

## Self-review

Performed against `docs/superpowers/specs/2026-05-23-test-framework-design.md`:

**Spec coverage:**
- §3.1 single file-based discovery → Task 6.2 (`discover_test_files`).
- §3.2 `Tester.describe` + instance methods → Tasks 4.1–4.3.
- §3.3 v1 matchers → Tasks 2.2–2.4 (`to_eq`, `not_to_eq`, `to_be_truthy`, `to_be_falsy`, `to_include`, `to_be_nil`, `not_to_be_nil`). **Gap:** `to_be_a(ClassName)` not implemented. Resolution: deferred to v1.1; runtime-class introspection requires a small addition to `std.core` not in scope.
- §3.4 discovery & naming → Task 6.2.
- §3.5 CLI → Task 6.1 (all flags present).
- §3.6 outputs — pretty/TAP/JSON → Tasks 6.4 / 7.1.
- §4.1 `std.test` package layout → Tasks 1.1–1.2 + 2.x + 3.1 + 4.x.
- §4.2 compile wrap → Task 6.3.
- §4.3 constraints (no top-level `def`/`use`; helpers in `tests/support/`) → Task 6.2 walker excludes `tests/support/`; documented in Task 7.3.
- §4.4 build pipeline → Task 6.4 (calls `compile::run`; the existing incremental cache is what `compile::run` consults).
- §4.5 execution → Tasks 6.4 / 6.5 (per-file parallel) + 5.2 (per-test fork in-binary).
- §4.6 panic catching via `ruxen_panic`+`abort` → Task 5.2.
- §4.7 cache integration → no explicit task — `compile::run` already keys by content+flags, so `ruxen test` inherits caching for free. Verify in Task 8.1 (incidentally; second `ruxen test` on the smoke project should be near-instant).
- §4.8 files touched — matches the File Structure list above.
- §5 test matrix → Tasks 4.1 / 4.3 / 5.2 / 5.3 cover the pass / fail / panic / fork-isolation matrix rows; CLI rows covered in 6.2 (`--list`), 6.4 (basic + cache), 6.5 (`--test-threads`), 7.1 (formats).
- §7 open questions:
  - OQ-1 `__caller_location` → deferred per spec recommendation (c).
  - OQ-2 `tests/support/` → Task 6.2 walker; Task 7.3 docs.
  - OQ-3 substring matching → **NOT v1** (Task 5.3 documents the cut).
  - OQ-4 thread cap → Task 6.5 (`min(ncpus, 8)`).
  - OQ-5 leak documentation → Task 7.3.
  - OQ-6 no top-level `def` → Task 7.3.

**Placeholder scan:**
- "TODO(post-v1)" in Task 5.3 child-side comment — intentional, marks the substring verification follow-up. Acceptable per spec §7 R-list.
- "Open follow-up" callout at end of Task 5.3 — intentional, mirrors spec recommendation.
- No vague "add error handling" / "implement later" / "similar to Task N" instances detected.

**Type consistency:**
- `Runner.handle_addr` referenced in Tasks 4.1, 4.2, 4.3 — defined in Task 4.1's Runner spec.
- `Runner.box_store` / `Runner.box_load` introduced as the fallback in Task 4.1 step 4 implementation note. The synthesised wrapper in Task 6.3 calls `Runner.box_store(r.handle_addr)` — matches.
- `TestFileResult` defined in Task 6.4, used in Task 6.5 worker pool and Task 7.1 renderers — fields consistent (`test_path`, `passed`, `failed`, `pending`, `stdout`, `stderr`, `exit_ok`).
- `TestOptions` fields used in Task 6.1 (definition site) and 6.2/6.4/6.5 (consumers) — all match.

**Known scope contractions vs spec:**
1. `to_be_a(ClassName)` — deferred (gap above).
2. `expect_panic` inline shape → replaced with `it_panics(name, substr)` (Task 5.3) for v1 implementability. Substring verification deferred. Tutorial (Task 7.3) documents the change.
3. Per-`it` fork isolation works inside one binary (Task 5.2); per-FILE parallel runs across binaries (Task 6.5). Both layers of isolation/parallelism the spec asks for are present.

---

## Execution

Plan complete and saved to `docs/superpowers/plans/2026-05-23-test-framework.md`.
