# Tier 3.08 — Property Testing (Claim Audit + Recommendation)

Status: draft
Depends on: none
Blocks: nothing; this is a documentation-correctness + coverage item

---

## 1. Summary & motivation

`CLAUDE.md` (the workspace-level instructions file at
`/home/sheraz/Documents/riven/CLAUDE.md`) claims:

> The project uses `proptest` for property-based testing in `riven-core`.

This doc verifies that claim and recommends an action. Ground truth:
the dependency *exists* but is effectively unused. Of the two
`proptest!` blocks in the codebase, both assert trivial tautologies
that are not property tests in any useful sense. This doc proposes
two paths (fix or delete) and recommends the fix path with a concrete
v1 property set.

---

## 2. Current state (grep-verified)

### 2.1 Dependency declaration

`crates/riven-core/Cargo.toml:16-17`:

```
[dev-dependencies]
proptest = "1"
```

Present. No workspace-level `proptest` in `Cargo.toml`, just the
`riven-core` dev-dep.

### 2.2 Usage sites — two and only two

Grep `use proptest|proptest!\s*\{|proptest::` across the whole repo:

```
crates/riven-core/tests/runtime_safety.rs:61:    use proptest::prelude::*;
crates/riven-core/tests/runtime_safety.rs:68:    proptest! {
```

One file, one `proptest!` block. That block contains two tests
(`runtime_safety.rs:68-86`):

```rust
proptest! {
    /// Verify that string concatenation produces the expected length.
    #[test]
    fn concat_length(a in "[a-z]{0,50}", b in "[a-z]{0,50}") {
        let expected_len = a.len() + b.len();
        // This is a compile-time validation that the runtime is sound.
        // The actual concat happens in the C runtime.
        prop_assert!(expected_len <= 100);
    }

    /// Verify that vec operations maintain invariants across many sizes.
    #[test]
    fn vec_size_invariant(n in 0usize..100) {
        // A vec with n pushes should have len == n.
        // This tests the vec_push/vec_len contract.
        prop_assert_eq!(n, n);
    }
}
```

**Neither test exercises Riven code.**

- `concat_length` generates two strings of length 0-50, adds their
  lengths, and asserts the sum ≤ 100. Trivially true by
  construction. No call into `riven_string_concat` or any Riven
  runtime function.
- `vec_size_invariant` generates an integer `n` in 0..100 and asserts
  `n == n`. Trivially true. No call into `riven_vec_push` / `riven_vec_len`.

The inline comments acknowledge this ("This is a compile-time
validation... the actual concat happens in the C runtime" — but the
test doesn't actually call the C runtime).

### 2.3 What the CLAUDE.md claim implies vs reality

Claim: "property-based testing in riven-core."
Reality: two placeholder tests that assert tautologies. Property
testing as an engineering practice is not happening.

### 2.4 Other Rust tests (for context)

There are ~400 `#[test]` occurrences across the codebase (grep count
of 410 noted during research). Those are all example-based unit and
integration tests, not property tests. The test suite is reasonably
strong for a ~20k-line compiler — the gap is specifically in
property-shaped assertions.

---

## 3. Goals & non-goals

### Goals

1. Either remove the CLAUDE.md claim or back it with real coverage.
2. If fixing: ship a v1 property set that exercises the compiler's
   most-tested-via-properties surfaces (lexer, parser, formatter).
3. Each property must have a clear hypothesis, a generator, and a
   failure mode that would indicate a real bug.
4. Integrate into `cargo test` without adding CI time > 30 seconds.

### Non-goals

- **Property testing for user Riven code.** That's a tier-4 item
  (if we ever add a `@[proptest]` attribute). Out of scope here.
- **Fuzzing.** `cargo fuzz` + libfuzzer is complementary; this doc
  specifies proptest-style randomized testing inside the standard
  `cargo test` flow, not continuous fuzzing.
- **Random-program generation that must typecheck.** Generating
  well-typed Riven programs is a full research problem (see
  `csmith` for C). v1 focuses on round-trip / panic-freedom
  properties, which work with arbitrary byte streams.

---

## 4. Recommendation

**Option A: Remove the claim.**
Delete the sentence from `CLAUDE.md`. Remove `proptest` from
`Cargo.toml`. Delete `runtime_safety.rs:57-87`. Zero engineering
effort, restores accuracy.

**Option B: Add real coverage.** (Recommended.)
Replace the two placeholder tests with three to six real property
tests in the areas where they'd actually catch bugs. Keep the
`proptest` dependency, keep the claim. ~3-5 days of engineering.

This doc specifies Option B.

---

## 5. Properties to ship

### 5.1 P1 — Lexer never panics on arbitrary bytes

**Hypothesis.** For any byte string of length ≤ 1 KB,
`Lexer::new(&source).tokenize()` returns either `Ok(tokens)` or
`Err(diagnostics)` — never panics, never hangs.

**Generator.** `proptest::collection::vec(any::<u8>(), 0..1024)` →
UTF-8 lossy.

**Rationale.** A panic in the lexer is a P0 bug — it breaks LSP
(doc 01), the formatter (`riven fmt`), and every user's IDE.
Fuzzing the lexer is one of the highest-value property tests.

**Expected failure.** If the lexer ever hits an
`unreachable!()`, `unwrap()` on a genuine None, or an infinite loop
on malformed input, proptest will minimize and report a reduced
reproducer.

### 5.2 P2 — Parser never panics on arbitrary token streams

**Hypothesis.** For any `Vec<Token>` built from a bounded alphabet,
`Parser::new(tokens).parse()` returns either `Ok(program)` or
`Err(diagnostics)` — never panics, never infinite-loops.

**Generator.** `proptest::collection::vec(arb_token(), 0..100)` where
`arb_token()` picks from a curated set of representative token kinds
(identifiers, operators, keywords, literals, punctuation).

**Rationale.** Same as lexer — parser panics break LSP. Harder to
generate cleanly (tokens are a richer alphabet) but higher value.

### 5.3 P3 — Formatter roundtrip is a fixpoint

**Hypothesis.** For any valid Riven program `src`:

```
format(src) = format(format(src))
```

That is, running the formatter twice produces identical output.

**Generator.** Either (a) a corpus of existing fixtures
(`crates/riven-core/tests/fixtures/*.rvn`) — then this is not strictly
a property but a deterministic roundtrip; or (b) a generator that
produces random well-formed Riven AST via a recursive-descent
proptest strategy.

**Rationale.** Fixpoint formatting is a widely-expected property
(rustfmt, gofmt, prettier all guarantee it). Breakage is usually
silent — users might not notice for months. Already partially covered
by `formatter/tests.rs` (grep said ~21 test functions there), but
property coverage catches generative edge cases those tests miss.

**Starter implementation:** Strategy (a) is cheap — iterate the
fixtures and assert. Strategy (b) is harder but more valuable.
Recommend both: (a) immediately, (b) in a follow-up.

### 5.4 P4 — Parse then unparse preserves semantics

**Hypothesis.** For any successfully-parsed program:

```
typecheck(parse(unparse(parse(src)))) == typecheck(parse(src))
```

Where "equals" compares the set of diagnostic codes and the shape of
the resulting HIR.

**Generator.** Corpus-based (same fixtures as P3).

**Rationale.** Tests that the formatter doesn't subtly alter program
meaning. If `unparse` emits `1 + 2 * 3` as `(1 + 2) * 3`, typeck
results diverge. Catches formatter precedence bugs.

### 5.5 P5 — Cache-key determinism

**Hypothesis.** For any source string `s`, compiler version `v`, and
flags `f`:

```
CacheKey::compute(s, v, f) == CacheKey::compute(s, v, f)
```

Stronger form: two equivalent inputs produce the same key regardless
of invocation order, regardless of environment variables.

**Generator.** Random strings + flag sets.

**Rationale.** The content-addressed cache (`rivenc/src/cache/hash.rs`)
underlies correctness of incremental builds. A non-deterministic key
means false cache hits → silent miscompilation. Worth guarding.

### 5.6 P6 — Borrow-check decision stability

**Hypothesis.** For any valid Riven program `src`:

```
borrow_check(typeck(parse(src))) == borrow_check(typeck(parse(src)))
```

Running borrow-check twice on the same input produces the exact same
set of errors in the same order.

**Generator.** Corpus-based.

**Rationale.** Non-deterministic error ordering is a real bug source
in borrow checkers (iteration over `HashMap` caught rustc for years).
Easy to test; catches a whole class of bugs cheaply.

### 5.7 Deferred to v2

- **P7 — Semantic preservation under MIR opts.** Run a program at
  opt-level 0 and opt-level 2; assert identical stdout / exit code.
  Depends on doc 07; defer.
- **P8 — Formatter idempotence under random editing.** Apply a
  formatter, randomly perturb whitespace, re-format, assert fixpoint.
  Harder; defer.

---

## 6. Implementation plan

### Files to touch

| File | Change |
|---|---|
| `crates/riven-core/tests/runtime_safety.rs:57-87` | Delete placeholder proptest block |
| `crates/riven-core/tests/proptest_lexer.rs` *new* | P1 tests (~50 lines) |
| `crates/riven-core/tests/proptest_parser.rs` *new* | P2 tests (~80 lines, includes `arb_token` strategy) |
| `crates/riven-core/tests/proptest_formatter.rs` *new* | P3 + P4 tests (~100 lines) |
| `crates/riven-core/tests/proptest_cache.rs` *new* (or in `rivenc/tests/`) | P5 tests (~40 lines) |
| `crates/riven-core/tests/proptest_borrow.rs` *new* | P6 tests (~50 lines) |
| `crates/riven-core/Cargo.toml` | (no change — `proptest` already declared) |
| `CLAUDE.md` | Update claim to be accurate: "property-based testing for lexer, parser, formatter roundtrip, and cache keys via proptest" |
| `crates/riven-core/src/CLAUDE.md` (if present) | Same update |
| `README.md` or `CONTRIBUTING.md` | Brief note on running proptest: `cargo test -p riven-core --test proptest_*` |

### Phase breakdown

**Phase 1 — P1 lexer (0.5 day).**
Write the lexer property, run proptest, fix anything it catches.
This will surface bugs — expect it.

**Phase 2 — P2 parser (1 day).**
Build `arb_token()`, write the property, fix anything it catches.
Parsers are always full of `unwrap()` on dubious invariants; budget
time for fixes.

**Phase 3 — P3/P4 formatter (1 day).**
Corpus-based first; move to generator-based in v2.

**Phase 4 — P5 cache key (0.5 day).**
Largely mechanical.

**Phase 5 — P6 borrow-check (0.5 day).**
Corpus-based determinism check.

**Phase 6 — Update docs (0.5 day).**
CLAUDE.md + README.

Total: 4 engineer-days for full v1.

### CI impact

Proptest default is 256 cases per test. With 6 tests at ~10 ms/case
= ~15 seconds total. Acceptable. Run on every PR.

Optional: shrink case count to 64 on CI via `PROPTEST_CASES=64`
environment variable to keep CI fast, while local dev uses the
default 256.

---

## 7. Interactions with other tier-3 items

- **Doc 07 (MIR opts).** Property P7 (opt-0 vs opt-2 equivalence)
  lands when doc 07 ships. Very high-value — would catch any
  pass-level miscompile.
- **Doc 06 (incremental).** Property P6 (borrow-check determinism)
  composes with doc 06's "differential testing" recommendation
  (incremental vs fresh analysis agreement).
- **Doc 03 (test framework).** If `@[test]` lands, a future
  `@[test(proptest)]` annotation could expose proptest to user
  Riven code. v2+.
- **Doc 02 (debugger).** No interaction.
- **Doc 01 (LSP).** No direct interaction, but LSP benefits
  indirectly from a panic-free lexer/parser.

### Tier-1 dependencies

None. All properties test *existing* compiler phases.

---

## 8. Phasing

| Phase | Scope | Days |
|---|---|---|
| 1 | P1 lexer | 0.5 |
| 2 | P2 parser | 1 |
| 3 | P3/P4 formatter | 1 |
| 4 | P5 cache | 0.5 |
| 5 | P6 borrow-check | 0.5 |
| 6 | Doc updates | 0.5 |

---

## 9. Open questions & risks

1. **OQ-1 — Should P1/P2 report minimum reproducers.**
   Yes — that's proptest's default behavior. Confirm the output is
   readable (proptest emits a file with the failing case in
   `proptest-regressions/`).
2. **OQ-2 — How aggressive should the generators be?**
   Balance signal vs noise. Too-restrictive generators find nothing;
   too-permissive find trivially "bad" inputs that aren't really
   bugs (e.g. 1 MB files that crash the parser because of stack depth —
   a real but separate concern).
3. **OQ-3 — Shrinking integration.**
   Proptest's shrinking works well for `u8` / String / tuple
   generators. For `Vec<Token>`, the default shrinker is ok; may
   want custom shrink rules for high-quality reduced reproducers.
4. **R1 — Proptest will find bugs.**
   Budget time to fix them. This is the whole point — don't
   under-plan the Phase 1-2 days.
5. **R2 — Flaky tests on low-probability cases.**
   Proptest uses a seeded RNG; reproducibility is guaranteed. A
   regression test file captures failing cases
   (`proptest-regressions/`). CI should commit these.
6. **OQ-4 — Location of proptest regressions in repo.**
   `proptest-regressions/` lives next to the test file by default.
   Commit them? Recommend yes — tests stay reproducible across
   machines.
7. **OQ-5 — Unicode in lexer fuzzing.**
   Generating random UTF-8 via `"[\\u{0080}-\\u{ffff}]{0,100}"`
   regex would catch Unicode-related bugs. v1 scope: ASCII + random
   bytes. v2: broaden to arbitrary Unicode.
8. **OQ-6 — `arb_token` strategy scope.**
   Should it include all `TokenKind` variants, including `Error`?
   Recommend: exclude `Error` and `Eof` (emitted by the lexer, not
   fed back into the parser in practice). Include all others.
9. **R3 — Test isolation.**
   Proptest runs test cases sequentially per property by default.
   No state between cases. Good. Do not introduce any global state
   into the property tests.
10. **OQ-7 — Format of CLAUDE.md update.**
    Replace the one-liner with a two-line paragraph that links to
    this doc. Example:
    > Property-based testing via `proptest` covers the lexer, parser,
    > formatter roundtrip, cache keys, and borrow-check determinism.
    > See `docs/requirements/tier3_08_property_testing.md`.
11. **OQ-8 — Should `runtime_safety.rs` be deleted or repurposed?**
    The top of that file (`runtime_safety.rs:1-55`) has two
    legitimate tests (`runtime_compiles_with_strict_warnings` and
    `runtime_compiles_with_sanitizers`). Keep those; delete only
    the proptest block at `:57-87`.

---

## 10. Test matrix

The "test matrix" for a property-testing initiative is the properties
themselves. Coverage:

| Property | Failure mode it catches |
|---|---|
| P1 | Lexer panic on malformed bytes |
| P2 | Parser panic on malformed tokens |
| P3 | Formatter not idempotent |
| P4 | Formatter changes program meaning |
| P5 | Cache-key non-determinism |
| P6 | Borrow-check order dependency |

Each property lives in its own `tests/proptest_*.rs` file. Each file
is runnable independently (`cargo test -p riven-core --test
proptest_lexer`). Running all six: `cargo test -p riven-core --test
'proptest_*'`.

One smoke test confirms the property-test infrastructure works:

```rust
// crates/riven-core/tests/proptest_smoke.rs
#[test]
fn proptest_framework_available() {
    use proptest::prelude::*;
    proptest!(|(n in 0usize..10)| {
        prop_assert!(n < 10);
    });
}
```

This is the only tautology we keep, and it's explicitly a smoke test.

---

## 11. Decision

**Recommend Option B (add real coverage, ~4 days).** The dependency
is already present; removing it and re-adding later costs more than
just doing the work. The lexer and parser are exactly the kind of
surface where property tests have historically found high-value bugs,
and Riven's are young enough that bugs are likely present.

If the project lead prefers Option A, the minimal edits are:

- Delete `crates/riven-core/tests/runtime_safety.rs:57-87`.
- Delete `crates/riven-core/Cargo.toml:16-17` (the `proptest` dev-dep).
- Update `CLAUDE.md` to drop the claim.
- Total effort: 5 minutes.

Either way, leave the codebase in a state where CLAUDE.md's claims
are accurate.
