# Tier 4.06 — Continuous Integration

## 1. Summary & Motivation

Riven has a release workflow (`.github/workflows/release.yml`, 144 lines) that fires on `v*` tags and ships prebuilt binaries. **There is no other CI.** Every PR today is merged on trust. `cargo test` is run by contributors on their local machines or not at all. `cargo fmt` isn't enforced. `cargo clippy` isn't enforced. There is no coverage metric, no fuzz target, no MSRV check, no security audit, no cross-compile smoke test. The root `Cargo.toml` has no `rust-version` field (`Cargo.toml:1-4`); a contributor can land code that only compiles on nightly and *nothing catches it* until a user tries to install.

This document specifies a single `ci.yml` workflow that ships *before* any other tier-4 subsystem lands. It's the cheapest, highest-leverage piece of tier-4 work: one week of engineering saves months of regression hunting.

## 2. Current State

### 2.1 Workflows present

```
.github/workflows/
└── release.yml       # build matrix, tags-only, 144 lines
```

No other workflow files.

### 2.2 `release.yml` (for reference)

Triggers on `push: tags: v*` and `workflow_dispatch`. Matrix of 4 triples (x86_64-linux, aarch64-linux via `cross`, x86_64-macos, aarch64-macos). Runs `cargo test --release --target ${TARGET}` on native matrix entries — *but only the `installed_binary` / `installed_pkg_manager` / `installed_repl` tests*, not the full suite:

```yaml
- name: Run installed-binary tests (native only)
  if: matrix.cross != true
  shell: bash
  run: |
    cargo test --release --target ${{ matrix.target }} \
      -p rivenc   --test installed_binary \
      -p riven-cli --test installed_pkg_manager \
      -p riven-repl --test installed_repl
```

So the *full* test suite never runs in CI. On anyone's machine.

### 2.3 Linting / formatting

- `rivenc fmt` exists (`crates/rivenc/src/main.rs:28`) — the Riven source formatter.
- `rivenc fmt --check` exists for CI mode.
- But `cargo fmt` on the Rust code, `cargo clippy` on the Rust code — not enforced.

### 2.4 Coverage / fuzzing

Neither exists. `Cargo.lock` shows `proptest` is a `[dev-dependencies]` entry in `riven-core/Cargo.toml:14` — used for property testing in typeck/lexer, but no fuzz harness.

### 2.5 MSRV

No `rust-version` anywhere. `crates/riven-core/Cargo.toml:4` is `edition = "2021"` — every crate matches. Some deps (cranelift 0.130) require recent Rust; `inkwell 0.5` with `llvm18-0` requires Rust 1.76+.

### 2.6 Security

No `cargo audit` / `cargo deny` CI. No `SECURITY.md` (doc 08).

### 2.7 Branch protection

Unknown from repo inspection; typically configured via GitHub UI. Assume nothing is protected. Recommendation: after `ci.yml` lands, configure branch protection on `master` to require CI green + 1 review.

## 3. Goals & Non-Goals

### Goals

1. A `ci.yml` workflow running on every PR and every push to `master`.
2. Jobs: build, test (full suite, workspace-wide), `cargo fmt --check`, `cargo clippy -D warnings`, MSRV build, LLVM-feature build, coverage (informational), fuzz (time-bounded), cross-compile smoke (aarch64-linux-gnu at minimum).
3. Ship an MSRV of `rust-version = "1.78"` at the workspace root.
4. A scheduled fuzz-extended run (1h per target on `workflow_dispatch` or nightly cron).
5. A release-readiness check: `cargo publish --dry-run` on each crate (once registries for Rust crates make sense — today, just package-checks).
6. A security audit: `cargo audit` in a matrix entry.
7. A dependency-review workflow via GitHub's built-in action.
8. No flaky tests in the default path. Tests that need a toolchain component (LLVM, `wasmtime`, `qemu`) are explicitly gated and use `continue-on-error: true` until the tooling stabilizes.
9. Reasonable runtime: full CI completes in < 10 minutes on typical PR changes (cache-enabled).

### Non-Goals

- Full build-artifact CI for every commit (the release workflow is separate).
- Cross-matrix for every triple (doc 02 drives that).
- A private self-hosted runner.
- Cost optimization (parallel billing). GitHub Actions is free for public repos.
- Per-PR deployment previews (no web surface to preview).
- Merge queues (can be added later via `bors`/`mergify` if contention becomes a problem).

## 4. Surface

### 4.1 Workflow files

```
.github/workflows/
├── ci.yml                # PR + push-to-master; fast, required
├── fuzz.yml              # scheduled + workflow_dispatch; long-running
├── audit.yml             # scheduled; advisory DB
├── coverage.yml          # PR + push-to-master; informational
└── release.yml           # unchanged
```

### 4.2 `ci.yml` job matrix

```yaml
name: CI

on:
  pull_request:
    branches: [master]
  push:
    branches: [master]

# Cancel in-progress CI when a new commit is pushed to the same branch.
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  test:
    name: Test (${{ matrix.os }}, ${{ matrix.rust }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-14]
        rust: [stable, "1.78"]                              # current + MSRV
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ matrix.rust }}
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - name: Build
        run: cargo build --workspace --all-targets
      - name: Test
        run: cargo test --workspace --all-targets
      - name: Doc tests
        run: cargo test --workspace --doc

  test-llvm:
    name: Test with LLVM backend
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install LLVM 18
        run: |
          wget -qO- https://apt.llvm.org/llvm.sh | sudo bash -s -- 18
          echo "LLVM_SYS_180_PREFIX=/usr/lib/llvm-18" >> "$GITHUB_ENV"
      - uses: Swatinem/rust-cache@v2
      - name: Test with llvm feature
        run: cargo test --workspace --features llvm

  lint:
    name: Lint (fmt + clippy)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - name: cargo fmt --check
        run: cargo fmt --all --check
      - name: cargo clippy -D warnings
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: rivenc fmt --check on fixtures
        run: |
          cargo build -p rivenc
          ./target/debug/rivenc fmt --check crates/riven-core/tests/fixtures/

  cross:
    name: Cross-compile (${{ matrix.target }})
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: aarch64-unknown-linux-gnu
          # wasm32-wasi added once doc 03 phase 3a lands
          # - target: wasm32-wasi
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Install cross
        run: cargo install cross --locked
      - uses: Swatinem/rust-cache@v2
      - name: Build with cross
        run: cross build --workspace --target ${{ matrix.target }}

  fuzz-smoke:
    name: Fuzz smoke (60s per target)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly                # cargo-fuzz requires nightly
      - uses: Swatinem/rust-cache@v2
      - name: Install cargo-fuzz
        run: cargo install cargo-fuzz --locked
      - name: Fuzz lexer (60s)
        working-directory: fuzz
        run: cargo fuzz run lexer -- -max_total_time=60
      - name: Fuzz parser (60s)
        working-directory: fuzz
        run: cargo fuzz run parser -- -max_total_time=60

  msrv-check:
    name: MSRV ({{ matrix.rust }})
    runs-on: ubuntu-latest
    strategy:
      matrix:
        rust: ["1.78"]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ matrix.rust }}
      - uses: Swatinem/rust-cache@v2
      - name: cargo check --workspace
        run: cargo check --workspace --all-targets
```

### 4.3 `fuzz.yml` — long fuzzing

```yaml
name: Fuzz (extended)

on:
  schedule:
    - cron: '30 2 * * *'                                     # 02:30 UTC daily
  workflow_dispatch:
    inputs:
      duration:
        description: 'Seconds per target'
        default: '3600'

jobs:
  fuzz:
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        target: [lexer, parser, typeck]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - uses: Swatinem/rust-cache@v2
      - name: Install cargo-fuzz
        run: cargo install cargo-fuzz --locked
      - name: Fuzz
        working-directory: fuzz
        run: |
          DURATION=${{ github.event.inputs.duration || 3600 }}
          cargo fuzz run ${{ matrix.target }} -- -max_total_time=$DURATION
      - name: Upload corpus on failure
        if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: fuzz-corpus-${{ matrix.target }}-${{ github.run_id }}
          path: fuzz/artifacts/${{ matrix.target }}/
```

### 4.4 `audit.yml` — cargo-audit

```yaml
name: Audit

on:
  schedule:
    - cron: '0 6 * * 1'                                      # Mondays 06:00 UTC
  push:
    paths:
      - 'Cargo.lock'
      - '**/Cargo.toml'
  workflow_dispatch:

jobs:
  security-audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rustsec/audit-check@v2
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
```

### 4.5 `coverage.yml` — informational

```yaml
name: Coverage

on:
  pull_request:
  push:
    branches: [master]

jobs:
  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: llvm-tools-preview
      - uses: taiki-e/install-action@cargo-llvm-cov
      - uses: Swatinem/rust-cache@v2
      - name: Generate coverage
        run: cargo llvm-cov --workspace --lcov --output-path lcov.info
      - name: Upload to Codecov
        uses: codecov/codecov-action@v4
        with:
          files: lcov.info
          fail_ci_if_error: false
```

### 4.6 MSRV pin

`Cargo.toml` at the workspace root grows:

```toml
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
rust-version = "1.78"
edition = "2021"
```

Each member crate then references `rust-version.workspace = true` (requires the `workspace.package` inheritance from tier 4.01 §4.1 workspace support — for now, duplicate the line in every crate's `[package]`).

### 4.7 Fuzz harness layout

```
fuzz/
├── Cargo.toml
├── .gitignore                 # /target, /artifacts, /corpus
├── fuzz_targets/
│   ├── lexer.rs
│   ├── parser.rs
│   └── typeck.rs
└── corpus/
    ├── lexer/                 # seed corpus from tests/fixtures/
    ├── parser/
    └── typeck/
```

Example `fuzz_targets/lexer.rs`:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use riven_core::lexer::Lexer;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut lex = Lexer::new(s);
        let _ = lex.tokenize();
    }
});
```

Example `fuzz_targets/parser.rs`:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut lex = Lexer::new(s);
        if let Ok(tokens) = lex.tokenize() {
            let mut p = Parser::new(tokens);
            let _ = p.parse();
        }
    }
});
```

Example `fuzz_targets/typeck.rs`:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut lex = riven_core::lexer::Lexer::new(s);
        if let Ok(tokens) = lex.tokenize() {
            let mut p = riven_core::parser::Parser::new(tokens);
            if let Ok(prog) = p.parse() {
                let _ = riven_core::typeck::type_check(&prog);
            }
        }
    }
});
```

## 5. Architecture / Design

### 5.1 Job dependencies

`ci.yml` jobs run in parallel. No explicit `needs:` except as specific blockers require. Failure of `fuzz-smoke` is reported but does not block merge by default — graduate to required once fuzzing is stable (§9 Q5).

### 5.2 Cache strategy

`Swatinem/rust-cache@v2` is the community standard. It caches `target/`, `~/.cargo/registry/cache/`, `~/.cargo/registry/index/`, `~/.cargo/git/db/`. Typical speedup: 50%+ on warm runs.

Cache key includes: os, rust version, `Cargo.lock` hash. Invalidated automatically when deps change.

### 5.3 MSRV strategy

Two approaches:

- **Pin absolutely** (`rust-version = "1.78"`) and bump manually. This is what we recommend.
- **Track with a cadence** (e.g. always 2 versions behind stable).

Pinning absolutely is Cargo's policy (`rust-version` in every crate that cares). Bump at minor releases (v0.2, v0.3). Document in CONTRIBUTING.md.

### 5.4 Flakiness mitigation

CI flakes erode trust. Standing rules:

1. **No network in tests.** Tests that touch the network are tagged `#[ignore]` and run in a separate job (for `riven-cli/tests/registry_mock.rs`, doc 01).
2. **No `#[cfg(target_os = "linux")]` assumptions.** Tests must work on both Linux and macOS or be explicitly gated.
3. **Temp files in `std::env::temp_dir()` with unique names.** Already the pattern (`crates/riven-cli/src/scaffold.rs:225-231`).
4. **No real git clones from `https://github.com/…` in tests.** Use `file://` local repos (tier 4.01 mock registry).
5. **Timeouts on all async ops.** If a test hangs, fail fast with `cargo test --timeout 120`.

### 5.5 Required vs informational checks

Branch protection gates on **required** statuses. Recommend the following as required:

- `test / ubuntu-latest / stable`
- `test / macos-14 / stable`
- `test-llvm / Test with LLVM backend`
- `lint / Lint (fmt + clippy)`
- `msrv-check / MSRV (1.78)`

Informational (not required):

- `coverage / coverage`
- `cross / Cross-compile (aarch64-unknown-linux-gnu)` (until it's proven stable)
- `fuzz-smoke / Fuzz smoke` (until it's proven stable)
- `audit / security-audit` (manually review weekly)

### 5.6 CI runtime target

Goal: < 10 minutes wall-clock for a typical PR. Current full `cargo test --workspace` is ~3-5 minutes locally on a modern laptop; GitHub's free runners are ~50% that speed. With cache, budget:

- `test / stable`: 5-7 min.
- `test / 1.78`: 5-7 min.
- `test-llvm`: 7-10 min (LLVM install adds 2 min).
- `lint`: 2-3 min.
- `cross`: 5-7 min.
- `fuzz-smoke`: 2 min (60s × 2 targets + overhead).
- `msrv-check`: 3-4 min.
- `coverage`: 5-7 min (runs in parallel).

All parallel. Longest single job dictates wall time.

## 6. Implementation Plan — files to touch

### New files

- `.github/workflows/ci.yml` — the main workflow.
- `.github/workflows/fuzz.yml` — extended fuzz runs.
- `.github/workflows/audit.yml` — cargo-audit.
- `.github/workflows/coverage.yml` — coverage upload.
- `fuzz/Cargo.toml` — nested cargo-fuzz workspace.
- `fuzz/fuzz_targets/lexer.rs`, `parser.rs`, `typeck.rs` — see §4.7.
- `fuzz/corpus/lexer/`, `parser/`, `typeck/` — seeded from `crates/riven-core/tests/fixtures/*.rvn`.
- `fuzz/.gitignore`.
- `codecov.yml` at repo root — coverage configuration (target %, status checks).
- `clippy.toml` at repo root — lint config (any allowlisted lints documented inline).
- `rustfmt.toml` at repo root — formatter config.

### Touched files

- `Cargo.toml` at repo root — add `[workspace.package] rust-version = "1.78"`.
- Every `crates/*/Cargo.toml` — add `rust-version = "1.78"` under `[package]` (or `rust-version.workspace = true` once tier 4.01 §4.1 lands).
- `README.md` — add MSRV line and CI badge.
- `.github/workflows/release.yml:56-66` — delete the subset-test invocation once `ci.yml` supersedes it (they duplicate work, and the release matrix doesn't need to re-run tests).
- `CONTRIBUTING.md` (doc 08) — document MSRV policy, PR checklist.

### Tests

CI is self-testing — if the workflow runs, it works. Add a canary test that intentionally fails with `cargo clippy -- -D warnings` (via a `#[allow(…)]`-gated block) to verify the lint gate trips. Delete after confirmation.

## 7. Interactions with Other Tiers

- **Tier 4.01 package manager.** Workspace inheritance lets `rust-version.workspace = true` replace per-crate duplication. `ci.yml` invokes `cargo test --workspace`, which exercises workspace logic.
- **Tier 4.02 cross-compilation.** The `cross` job in `ci.yml` is the first real test of the toolchain. Adds target smoke tests as each target lands.
- **Tier 4.03 WASM.** `ci.yml` should grow a `wasm32-wasi` matrix entry running `wasmtime run` on the compiled artifact once doc 03 phase 3a ships.
- **Tier 4.04 no_std.** `ci.yml` grows a no_std matrix entry (`cargo build --target <embedded-triple>` of a tiny fixture) once doc 04 phase 4c ships.
- **Tier 4.05 cbindgen.** `ci.yml` grows a cbindgen smoke test (`rivenc --emit=c-header … | gcc -fsyntax-only -`) once doc 05 phase 5a ships.
- **Tier 4.07 examples.** Every example in `examples/` gets compiled in CI via a matrix job `examples: strategy: matrix: example: [01-cli-utility, 02-tcp-echo, ...]`. Prevents examples from bitrotting.
- **Tier 4.08 repo hygiene.** `SECURITY.md` and `CODE_OF_CONDUCT.md` don't affect CI directly, but a dependency-review workflow (`actions/dependency-review-action@v4`) enforces that new deps in PRs don't add yanked/advisory'd crates.

## 8. Phasing

### Phase 6a — ship minimal CI (0.5 week)

1. `ci.yml` with `test` + `lint` + `msrv-check` only.
2. Pin `rust-version = "1.78"` in every crate.
3. Configure branch protection to require CI green on `master`.
4. **Exit:** every PR runs `cargo test --workspace` + `cargo fmt --check` + `cargo clippy -D warnings` on stable + 1.78, Linux + macOS.

### Phase 6b — LLVM + coverage + audit (0.5 week)

1. `test-llvm` matrix entry.
2. `coverage.yml` uploading to Codecov (informational).
3. `audit.yml` scheduled weekly.
4. **Exit:** LLVM backend is always exercised; coverage % appears on PRs; weekly email if a dep has a CVE.

### Phase 6c — fuzz harness (0.5 week)

1. `fuzz/` directory with lexer + parser targets.
2. Seed corpus from `tests/fixtures/`.
3. `ci.yml`'s `fuzz-smoke` job (60s per target).
4. `fuzz.yml` scheduled daily, 1h per target.
5. **Exit:** every PR runs a 2-minute fuzz smoke; nightly runs a 3-hour fuzz extended; any crash opens an artifact upload.

### Phase 6d — cross + examples (0.5 week, depends on doc 02 phase 2b and doc 07)

1. `cross` matrix entry for `aarch64-unknown-linux-gnu`.
2. `examples` matrix compiling each example.
3. **Exit:** every PR confirms aarch64 cross-compile works and no example is broken.

### Phase 6e — WASM + no_std (depends on doc 03, doc 04)

1. Add `wasm32-wasi` matrix entry (build + `wasmtime run`).
2. Add an embedded-fixture no-std build entry.

## 9. Open Questions & Risks

1. **GitHub-hosted runner cost on private forks.** Irrelevant until the repo is private. Free for public.
2. **Codecov dependency.** Codecov the vendor. Alternative: `cargo llvm-cov report --json-summary` + a jq-based check that coverage didn't regress > 1%. Recommend v1: Codecov (free for OSS, least friction). Reevaluate if it becomes a blocker.
3. **MSRV of 1.78.** Check that every dep supports 1.78. `cranelift-codegen 0.130` requires 1.74. `inkwell 0.5` requires 1.76. `clap 4` requires 1.74. `toml 0.8` requires 1.69. `tower-lsp` requires 1.70. 1.78 is conservative — should hold. Verify with `cargo msrv` before landing.
4. **Nightly dependency for cargo-fuzz.** cargo-fuzz requires nightly for LLVM sanitizer integration. We pin `nightly` in the fuzz job only — not the full CI. Acceptable cost. Alternative: `cargo-afl` (AFL++) which works on stable. Less integrated. Recommend cargo-fuzz + nightly.
5. **Flaky fuzz finds.** A 60s fuzz run can find a crash that's also a known issue. Recommend: introduce a `fuzz/corpus/<target>/known-issues/` directory of inputs explicitly allowed to crash; the job ignores those but alerts on novel crashes.
6. **Clippy warnings churn.** `cargo clippy -D warnings` can block unrelated PRs if Rust releases a new clippy. Two options:
   - **a)** Pin clippy to the MSRV (1.78's version). Stable, no churn.
   - **b)** Run against stable. Gets newer warnings sooner.
   - **Recommend (a):** use the 1.78 toolchain for clippy specifically. Bump clippy when bumping MSRV.
7. **Fmt churn.** Same story. `cargo fmt` output changes across Rust versions. Pin rustfmt to 1.78's.
8. **Test runtime balloon.** As tests grow, CI slows. Recommend: enforce a soft limit (`timeout 30` at the `cargo test` level) and open investigation if any single test > 5 seconds.
9. **LLVM install time.** `apt.llvm.org`'s script is slow (~2 min). Recommend: consider caching the LLVM install in a custom Docker image if it becomes a bottleneck.
10. **macOS runner time.** macOS runners are slower + billed at 10× rate internally (doesn't matter for public repos). Recommend: only run `test / macos / stable` — drop `test / macos / 1.78`. Linux covers MSRV.
11. **Windows.** No Windows in the matrix because Riven doesn't support Windows as a target (doc 02 §3). If/when it does, add.
12. **Required-status churn.** Every time we add a matrix entry, branch protection needs updating. Recommend: document in CONTRIBUTING.md, treat as expected maintenance.
13. **CODEOWNERS.** `@sherazp995` is currently the only human. Recommend shipping a `CODEOWNERS` file in `.github/` once a second maintainer joins.
14. **Dependency-review-action.** GitHub built-in, works on public repos. Recommend adding to `ci.yml` as an informational check.
15. **Secrets for Codecov.** `CODECOV_TOKEN` is repo secret. Document in CONTRIBUTING.md that new repos need this set.
16. **Merge queues.** Out of v1. Add via `bors` or GitHub's native merge queue once PR throughput warrants.

## 10. Acceptance Criteria

Phase 6a:

- [ ] `.github/workflows/ci.yml` exists and runs on every PR + every push to `master`.
- [ ] The `test` job runs `cargo test --workspace --all-targets` on Linux + macOS × stable + 1.78.
- [ ] The `lint` job runs `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] `cargo fmt --all --check` passes on the current master (one-time cleanup commit if needed).
- [ ] `cargo clippy -- -D warnings` passes on the current master (allowlist obvious false positives in `clippy.toml`).
- [ ] `Cargo.toml` at repo root has `rust-version = "1.78"` (duplicated per crate if workspace inheritance not yet landed).
- [ ] `cargo +1.78 check --workspace` succeeds.
- [ ] Branch protection on `master` requires `ci / test (ubuntu-latest, stable)` + `ci / lint` green.
- [ ] A PR with intentionally broken `cargo fmt` is blocked by CI.
- [ ] A PR with intentionally broken `cargo clippy` is blocked by CI.
- [ ] README gets a `[![CI](...badge url...)](...link...)` badge.

Phase 6b:

- [ ] `test-llvm` job installs LLVM 18, runs `cargo test --workspace --features llvm`.
- [ ] `coverage.yml` uploads an lcov.info to Codecov; a coverage % shows up in PR comments.
- [ ] `audit.yml` runs weekly; a PR that adds a yanked crate fails the check.

Phase 6c:

- [ ] `fuzz/` directory has lexer + parser + typeck targets.
- [ ] Seed corpus contains at least 13 fixture inputs.
- [ ] `ci.yml`'s `fuzz-smoke` runs 60s per target and passes on clean input.
- [ ] `fuzz.yml` runs nightly; manual dispatch with `duration=600` works.
- [ ] Inserting a deliberate `panic!` in the lexer (via a debug-only feature) causes `fuzz-smoke` to fail (verified once, then reverted).

Phase 6d:

- [ ] `cross` job successfully builds the workspace for `aarch64-unknown-linux-gnu`.
- [ ] `examples` matrix job compiles every directory under `examples/`.
- [ ] A broken example (deliberate) fails CI.

Phase 6e (on 4.03 / 4.04 landing):

- [ ] `wasm32-wasi` build + `wasmtime run` smoke test is green.
- [ ] `thumbv7em-none-eabihf` no_std build is green (or a chosen equivalent).

Global:

- [ ] Total PR wall-clock CI time < 10 minutes for a typical change.
- [ ] Zero flaky failures in the first 10 consecutive CI runs of the shipped workflow.
- [ ] CONTRIBUTING.md documents MSRV, CI expectations, and the PR checklist.
