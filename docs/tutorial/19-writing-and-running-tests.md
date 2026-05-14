# Writing and Running Tests

Riven uses **Spec-Driven Development** (see
[Chapter 20 — Specs & SDD](20-specs-and-sdd.md) for the workflow).
The TL;DR for tests: every spec behaviour is pinned by at least one
test in the Rust integration suite or as a release-e2e fixture.
This chapter shows you both layers — how to read the existing tests
and how to write your own.

---

## 1. The two test layers

| Layer             | Lives in                                      | Best for                                              |
|-------------------|-----------------------------------------------|-------------------------------------------------------|
| Integration tests | `crates/riven-core/tests/*.rs`                | Compile-and-run + assertions on typed Rust values     |
| Release-e2e       | `tests/release-e2e/cases/*.rvn` + `expected/` | Byte-exact stdout comparison across both backends     |

Integration tests run on every `cargo test --workspace` (≈1180 tests
on `v1-missing-features` as of writing).  Release-e2e fixtures run
under `cargo test --release ... -- --ignored` and on the post-merge
CI job (~220 fixtures, takes a few minutes locally).

---

## 2. Running everything

```bash
# Fast default suite — every cargo test in the workspace.
cargo test --workspace

# Just one test file:
cargo test -p riven-core --test stdlib_fmt_runtime

# Just one test fn:
cargo test -p riven-core --test stdlib_fmt_runtime -- \
    interpolation_float_precision

# Slow but comprehensive — compile-and-run every release-e2e fixture
# through the in-process pipeline:
cargo test --release -p riven-core --test release_e2e_smoke -- --ignored
```

The full-fixture run is gated behind `--ignored` so it doesn't run
in the default loop.  Run it before merging anything that touches
codegen or the runtime.

---

## 3. Writing a release-e2e fixture

Use these when you want a **byte-exact** stdout check.  Add two files:

```
tests/release-e2e/cases/NNN_<name>.rvn
tests/release-e2e/expected/NNN_<name>.out
```

`NNN_` is a free integer prefix used for ordering.  Pick a number
that doesn't collide with existing fixtures (highest currently in
use is around `611`).

**Example:** `tests/release-e2e/cases/071_interp_format_specs.rvn`:

```riven
# Phase 2 #06.D4 — width / align / fill / precision applied at runtime.

def main
  let n: Int = 42
  puts "[#{n:>5}]"
  puts "[#{n:<5}]"
  puts "[#{n:^6}]"

  let pi: Float = 3.14159
  puts "pi=#{pi:.2}"
  puts "[#{pi:>8.2}]"
end
```

`tests/release-e2e/expected/071_interp_format_specs.out`:

```
[   42]
[42   ]
[  42  ]
pi=3.14
[    3.14]
```

The harness diffs stdout against the `.out` file character-by-character.
A trailing newline difference is meaningful — preserve it exactly.

To run just your new fixture during development, write an inline
cargo test (see §4) that compiles+runs the same source; the
release-e2e harness will then pick up the fixture automatically
when you next run the full suite.

---

## 4. Writing an integration test

Use these when you need to assert on values, behaviour, or
diagnostics that aren't visible from stdout.

**Pattern:** parse → typecheck → MIR-lower → codegen → run → assert.
The `compile_and_run` helper exists in most `stdlib_*.rs` files;
copy-paste it.

```rust
// crates/riven-core/tests/my_feature.rs
use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

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
    assert!(result.diagnostics.iter().all(|d|
        d.level != riven_core::diagnostics::DiagnosticLevel::Error),
        "typecheck errors: {:?}", result.diagnostics);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer.lower_program(&result.program).expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn my_feature_does_the_thing() {
    let source = r##"
def main
  puts "expected output"
end
"##;
    let (stdout, _stderr, ok) = compile_and_run(source, "my_feature_thing");
    assert!(ok);
    assert_eq!(stdout, "expected output\n");
}
```

That's the whole pattern.  Drop the file in
`crates/riven-core/tests/`, add `#[test]` fns, and `cargo test`
picks it up automatically — no separate registration.

---

## 5. Linking tests back to specs

When you add a behaviour to a spec, follow the existing convention:
list each pin test in the spec's "Pin tests" table.  Future readers
should be able to trace any `B<n>` to the exact `fn` that pins it.

**Don't** write tests that aren't named in a spec — the SDD
workflow says spec first, test second.  If you need a test for
behaviour that isn't yet spec'd, either extend the spec or write
the test as a TODO and revisit.

When you find a behaviour in a spec's **Gaps** section, that's an
invitation: write the pin test, then move the line out of "Gaps"
and into the "Pin tests" table.  See commits 0a8c499 and `4f...`
for examples of this workflow in action (env, fs, process, vec gap
fills).

---

## 6. Diagnostics tests

For typeck rejections, use `typeck::type_check(&program)` directly
and inspect `.diagnostics`.  See `crates/riven-core/tests/implicit_negatives.rs`
for the canonical pattern: compile-only (no codegen) and assert on
the diagnostic `code` (e.g. `"E0610"`) rather than the message
text.  Error-message wording can change; codes are stable.

```rust
let diagnostics = typecheck_source(src);
let codes: Vec<&str> = diagnostics
    .iter()
    .filter(|d| d.level == DiagnosticLevel::Error)
    .filter_map(|d| d.code.as_deref())
    .collect();
assert!(codes.contains(&"E0610"), "expected E0610, got {:?}", codes);
```

---

## 7. Parser-only tests

If you're adding new syntax that doesn't yet have semantics, you
only need the lexer + parser.  See
`crates/riven-core/tests/const_generics.rs` for the pattern:

```rust
use riven_core::lexer::Lexer;
use riven_core::parser::ast::{GenericParam, Program, TopLevelItem};
use riven_core::parser::Parser;

fn parse(src: &str) -> Program {
    let mut lx = Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    p.parse().expect("parse")
}

#[test]
fn my_new_syntax_parses() {
    let prog = parse("struct Foo[const N: USize] field: USize end");
    // Walk prog.items, assert on AST shape...
}
```

This is the right scaffold for SDD Stage-1 tests: pin the parser
surface red before any semantic work.

---

## 8. Common pitfalls

- **Forgetting `--release` on release-e2e.** The fixture runner is
  marked `#[ignore]` because it takes ~5 minutes; the `--release`
  flag speeds compile-and-run roughly 5× — without it the suite
  takes ~20 minutes locally.
- **Calling `Stdout.new()` instead of using `puts`.** Both work, but
  `puts s` is the idiomatic shortcut in test programs.  `Stdout.new()`
  has its place when you need `write_str` (no trailing newline).
- **Asserting on exact stdout including newlines.** Use
  `assert_eq!(stdout, "expected\n")` to catch missing / extra
  newlines — `assert!(stdout.contains(...))` will mask them.
- **Tests that touch the filesystem.** Use the `unique_tmp_dir`
  helper pattern from `stdlib_fs.rs` so parallel test runs don't
  race.  Always clean up on success and failure.
- **Network tests.** Bind on `127.0.0.1:0` so the kernel picks an
  unused port; pass the port to the Riven binary via env var
  (`RIVEN_NET_TEST_PORT` is the existing convention).  See
  `stdlib_net.rs::tcp_loopback_roundtrip`.

---

**Next:** [Chapter 20 — Specifications and SDD Workflow](20-specs-and-sdd.md)
to learn the why behind the patterns in this chapter.
