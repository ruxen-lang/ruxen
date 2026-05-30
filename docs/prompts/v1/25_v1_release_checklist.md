# 25 — v1.0.0 release checklist

**Depends on:** prompts 01-24 all complete.

## Pre-release

- [ ] All 24 prior prompts marked done.
- [ ] `cargo test --workspace` green on master.
- [ ] CI green on Ubuntu-latest, macOS-14, MSRV 1.91.
- [ ] No `#[ignore]` test additions.
- [ ] No `ruxen_noop_passthrough` for any user-callable method.
- [ ] Every error code emitted is in `diagnostics::codes::REGISTRY`
      (the registry test enforces this).
- [ ] Every E-code has a `docs/errors/E<NNNN>.md` long-form file.
- [ ] Language reference (`docs/reference/`) green on
      mdBook build, all examples compile and run.
- [ ] All in-tree examples (`examples/01-..` through wherever 5+)
      build and run.
- [ ] `ruxen doc` builds the stdlib doc site without warnings.
- [ ] Benchmark suite runs and produces a baseline JSON committed at
      `bench/baselines/v1.0.0.json`.

## Smoke tests on release artifacts

- [ ] Build release tarball via `release.yml`; install on a clean VM
      per OS (linux-x64, linux-aarch64, darwin-x64, darwin-aarch64).
- [ ] `ruxen new hello && cd hello && ruxen run` works end-to-end.
- [ ] `ruxen test` finds and runs functions marked with the `test`
      in-body directive.
- [ ] `ruxen explain E0001` prints title + long-form.
- [ ] `ruxen doc` generates a doc site for the new project.
- [ ] LSP launches in VSCode extension and serves diagnostics +
      completion.

## Soundness audit

- [ ] Run a leak audit on every `crates/ruxen-core/tests/*`
      that exercises user code; `outstanding == 0` for all.
- [ ] Run a UB audit: build with `RUSTFLAGS="-Zsanitizer=address"`
      (or equivalent) and execute the e2e suite; no failures.
- [ ] Review every `unsafe` block in the Rust compiler for
      justification comments. nil should be undocumented.

## Docs

- [ ] CHANGELOG.md "Unreleased" section closed; tag at `v1.0.0`.
- [ ] README.md updated with "1.0 features" section.
- [ ] Migration guide `docs/migration/v0-to-v1.md` lists every
      breaking change since v0.1.0.
- [ ] License audit: every dependency in `Cargo.lock` has a
      compatible license (MIT, Apache-2.0, BSD).

## Tag and ship

- [ ] `git tag v1.0.0 -s` (signed).
- [ ] Push tag → triggers `release.yml`.
- [ ] Verify GitHub release artifacts.
- [ ] Announce on website + relevant fora.

## Post-release immediate

- [ ] Open milestones for v1.1 (bug fixes) and v2.0 (actor model
      library, multi-threaded async scheduler, specialization).
- [ ] Bump `master` to `1.1.0-dev`.
- [ ] Open the v2 actor prompt at `docs/prompts/v2/`.
