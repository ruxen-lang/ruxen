# Writing and Running Tests

Writing tests is how you keep code working as it grows. In Ruxen, a test is just a normal `.rx` file with a `def main` that runs your code and panics if anything is wrong. There's no special test framework — you call `assert_eq` (or whatever helper you write), it panics on a failure, and the program's non-zero exit code tells `ruxen run` or your CI system that the test failed. This chapter walks through the minimal pattern, then shows how to organise tests in a real project, share helper functions, and write microbenchmarks.

---

## 1. Your first test

Save this as `test_math.rx`:

```ruxen
def add(a: Int, b: Int) -> Int
  a + b
end

def main
  let result = add(2, 3)
  if result != 5
    panic!("expected 5, got #{result}")
  end
  puts "ok"
end
```

Run it:

```bash
ruxen run test_math.rx
```

Output:

```
ok
```

If you change `a + b` to `a - b` and re-run, the program panics with `expected 5, got -1` and exits non-zero. That's the entire mechanism: **panic on failure, exit zero on success.**

`panic!(msg)` is the underlying primitive. Helpers like `assert_eq` and `expect!` are thin wrappers around it.

## 2. A reusable `assert_eq` helper

Writing `if x != y then panic!(...)` over and over gets tedious. Pull it into a helper:

```ruxen
def assert_eq[T](actual: T, expected: T) -> nil
    where T: PartialEq,
          T: Display
  if actual != expected
    panic!("assertion failed: #{actual} != #{expected}")
  end
end

def add(a: Int, b: Int) -> Int
  a + b
end

def main
  assert_eq(add(2, 3), 5)
  assert_eq(add(0, 0), 0)
  assert_eq(add(-1, 1), 0)
  puts "ok"
end
```

The `where` clause says `T` needs `PartialEq` (so we can use `!=`) and `Display` (so we can interpolate it into the panic message). Most numeric and string types satisfy both.

> **Try it:** add a `assert_true(cond: Bool, label: &str)` helper to your file. Then use it to check `add(10, 5) > 0`.

## 3. Organising tests in a project

Once your project has a manifest (`Ruxen.toml`), the convention is to put one test file per area under a `tests/` directory:

```
my_app/
  Ruxen.toml
  src/
    main.rx
    lib.rx
  tests/
    string_helpers.rx
    parser.rx
    integration_round_trip.rx
```

Declare each one as a binary in `Ruxen.toml`:

```toml
[package]
name = "my_app"
version = "0.1.0"

[[bin]]
name = "string_helpers_test"
path = "tests/string_helpers.rx"

[[bin]]
name = "parser_test"
path = "tests/parser.rx"
```

Run individual tests:

```bash
ruxen run --bin string_helpers_test
ruxen run --bin parser_test
```

Or chain them in a shell script:

```bash
#!/usr/bin/env bash
set -e
for t in string_helpers_test parser_test integration_round_trip_test; do
  ruxen run --bin "$t"
done
echo "all tests passed"
```

The `set -e` line is important — it makes the script abort on the first failing test, so your CI picks up the non-zero exit code instead of barrelling on to the next one.

## 4. Sharing helpers across test files

Put `assert_eq` and friends in a module so every test file can use them:

```ruxen
# src/test_support.rx

use std.fmt.Display

def assert_eq[T](actual: T, expected: T) -> nil
    where T: PartialEq,
          T: Display
  if actual != expected
    panic!("assertion failed: #{actual} != #{expected}")
  end
end

def assert_true(cond: Bool, msg: &String) -> nil
  if !cond
    panic!("assertion failed: #{msg}")
  end
end
```

Then in each test file:

```ruxen
# tests/string_helpers.rx

use my_app.test_support.{assert_eq, assert_true}
use my_app.string_helpers.{capitalize}

def main
  assert_eq(capitalize(&"hello"), "Hello")
  assert_true(capitalize(&"").len == 0, &"empty input yields empty output")
  puts "ok"
end
```

## 5. Testing error paths with `Result`

When the code under test returns a `Result`, match on it explicitly:

```ruxen
match parse(&"3 + (2")
  Ok(_)  -> panic!("expected parse error on unbalanced input")
  Err(e) -> assert_eq(e.kind, ParseErrorKind.UnbalancedParen)
end
```

For the happy path, `unwrap!()` is the shortcut for "this should be `Ok`":

```ruxen
let parsed = parse(&"1 + 2").unwrap!()
assert_eq(parsed.value, 3)
```

## 6. Microbenchmarks with `ruxen bench`

Once a test passes, the natural next question is "how fast is it?" Ruxen ships a tiny benchmarking harness. A bench file is a `.rx` file with one or more `def bench_*` functions:

```ruxen
# benches/string_concat.rx

use std.bench.Bencher

def bench_string_concat(b: &var Bencher)
  b.iter(&"string_concat", { ||
    var s = String.new
    for _i in 0..100
      s.push_str(&"xx")
    end
    s.len
  })
end

def main
  var b = Bencher.new(1000)
  bench_string_concat(&var b)
end
```

Run it:

```bash
ruxen bench benches/string_concat.rx
ruxen bench benches/string_concat.rx --filter concat
ruxen bench benches/string_concat.rx --iter-hint 10000
```

The harness scales the iteration count automatically until each measurement runs for at least ~100 ms, then prints `iters | total ns | ns/iter`. Your closure must return an `Int`; the harness uses that to keep the optimiser from deleting the work.

## 7. Common mistakes

- **Calling `exit(0)` at the end of `main`.** Don't — falling off the end already exits with 0, and `exit` short-circuits any cleanup (like file close on drop). Save `exit(n)` for non-zero codes from deep in helpers.
- **Not printing anything on success.** A test that silently exits looks identical to one that crashed before reaching `puts`. Always end with `puts "ok"` (or similar) so you can tell from the output that the test actually ran.
- **Forgetting `set -e` in test scripts.** Without it, a failing test prints its error and the script merrily continues. Add it to every shell wrapper.
- **Mocking instead of using real I/O.** Ruxen doesn't have a mocking library. The idiomatic shape is: write to a unique tmp file (or use `std.env.current_dir`), read it back, assert. Clean up the path at the *start* of each run, not the end, so a previous crash doesn't leave state behind.

> **Try it:** add a second test case that intentionally fails (e.g. `assert_eq(add(2, 2), 5)`). Re-run with `ruxen run test_math.rx && echo PASS || echo FAIL`. What do you see?

---

## Recap

- A test is just a `.rx` file: assertions in `main`, panic on failure, non-zero exit on panic.
- `panic!(msg)` is the primitive; build `assert_eq` / `assert_true` helpers on top of it.
- Real projects use `tests/` with one binary per file declared in `Ruxen.toml`.
- Shared helpers live in a module like `src/test_support.rx`.
- `ruxen bench` runs microbenchmarks — auto-scales iterations and reports ns/iter.

**Next:** [Chapter 20 — Const Generics](20-const-generics.md).
