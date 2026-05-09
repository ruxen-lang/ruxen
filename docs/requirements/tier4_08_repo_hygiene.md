# Tier 4.08 — Repo Hygiene

## 1. Summary & Motivation

A newcomer opening a GitHub repo evaluates its seriousness in seconds. The signals they look for:

- A `LICENSE` file — do I have permission to use this code?
- A `README.md` — what is this, how do I install it?
- A `CONTRIBUTING.md` — can I help?
- A `CODE_OF_CONDUCT.md` — is this community safe to participate in?
- A `SECURITY.md` — how do I report vulnerabilities privately?
- A `CHANGELOG.md` — has this project shipped anything?
- Badges, issue templates, PR templates — does the maintainer care about quality?

Riven today has `README.md`. **Everything else is missing.** The release workflow cargo-cults a `cp LICENSE* "${STAGE}/" 2>/dev/null || true` that silently succeeds when there is no LICENSE (`.github/workflows/release.yml:96`), shipping releases with an unclear legal status. The README declares `## License\n\nTBD` (`README.md:294-296`).

This document is the smallest of the tier-4 docs because the actual work is mostly writing prose. But the decisions — which license, what the CoC says, how security disclosure works — are consequential and shape what the project is.

## 2. Current State

### 2.1 Present

- `/home/sheraz/Documents/riven/README.md` — 296 lines. Solid content; declares license as "TBD" (line 294-296).

### 2.2 Absent

- `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`.
- `CONTRIBUTING.md`.
- `CODE_OF_CONDUCT.md`.
- `SECURITY.md`.
- `CHANGELOG.md`.
- `.github/ISSUE_TEMPLATE/`.
- `.github/PULL_REQUEST_TEMPLATE.md`.
- `CODEOWNERS` (under `.github/` or root).
- Badges in `README.md` (CI status, crate version, license, rustc MSRV).

### 2.3 Author metadata

`crates/*/Cargo.toml` files have no `authors = [...]` field, no `license = "..."` field, no `repository = "..."` field, no `description = "..."` field. `cargo publish --dry-run` on any crate would warn about missing metadata. (Publishing them to crates.io is out of scope — they're internal-only — but we still want the metadata for searchability and consistency.)

### 2.4 The silent license trap

`.github/workflows/release.yml:96`:

```yaml
cp README.md LICENSE* "${STAGE}/" 2>/dev/null || true
```

If `LICENSE` doesn't exist, the command fails, `2>/dev/null` hides it, `|| true` swallows it, and the release tarball ships without a license file. Users who download Riven today receive ambiguous legal terms. **This is the single most urgent tier-4 issue.**

### 2.5 .gitignore

Scaffold generates a per-project `.gitignore` (`crates/riven-cli/src/scaffold.rs:154-159`):

```
/target
Riven.lock                       # only for library projects
```

No root-level `.gitignore` in the repo itself — but given the root doesn't have much except `target/`, that's probably fine. Double-check: if `target/` isn't gitignored at root, the workspace `target/` accidentally gets committed.

## 3. Goals & Non-Goals

### Goals

1. Dual-license the repo: **MIT OR Apache-2.0**, matching the Rust ecosystem's norm.
2. Ship `LICENSE-MIT`, `LICENSE-APACHE` at the repo root.
3. Update `README.md:294-296` with the real license line.
4. Ship `CONTRIBUTING.md` explaining how to set up a dev environment, run tests, submit PRs, the MSRV policy, and the commit message style.
5. Ship `CODE_OF_CONDUCT.md` — adopt Contributor Covenant 2.1 verbatim with project-specific contact info.
6. Ship `SECURITY.md` with private-disclosure email.
7. Ship `CHANGELOG.md` in Keep-a-Changelog format, starting at `v0.1`.
8. Issue + PR templates under `.github/`.
9. README badges: CI status, MSRV, license.
10. Populate `authors`, `license`, `repository`, `description` in every crate's `Cargo.toml`.
11. A root `.gitignore` if one doesn't exist.
12. A `CODEOWNERS` file (single maintainer today, grows with the project).

### Non-Goals

- Trademarks / brand guidelines (v2).
- A DCO / CLA requirement (adds friction; the license is enough).
- A "first-time contributors" label taxonomy (community matures into this).
- Governance docs (BDFL for now; TSC once there's a need).

## 4. Surface

### 4.1 File layout

```
/
├── LICENSE-MIT                                  # MIT text (see §5.2)
├── LICENSE-APACHE                               # Apache-2.0 text (see §5.2)
├── README.md                                    # updated (license section + badges)
├── CONTRIBUTING.md                              # new
├── CODE_OF_CONDUCT.md                           # new
├── SECURITY.md                                  # new
├── CHANGELOG.md                                 # new
├── .gitignore                                   # new (root-level, if missing)
├── CODEOWNERS                                   # new; or under .github/
├── .github/
│   ├── ISSUE_TEMPLATE/
│   │   ├── bug_report.md
│   │   ├── feature_request.md
│   │   └── config.yml
│   ├── PULL_REQUEST_TEMPLATE.md
│   └── workflows/
│       ├── ci.yml                               # doc 06
│       └── release.yml                          # existing
```

### 4.2 `LICENSE-MIT` and `LICENSE-APACHE` — verbatim

Both come from https://choosealicense.com/ / https://spdx.org. **Do not modify.** The license text is `Copyright (c) <year> <copyright holder>` fields that get filled with `2026 Sheraz` (or whatever the legal copyright is at the time of landing). Update yearly.

### 4.3 `README.md` — license section replacement

Replace lines 294-296:

```md
## License

Riven is dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE) ([http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))
- [MIT license](LICENSE-MIT) ([http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Riven by you, as defined in the Apache-2.0 license, shall be dual-licensed as above, without any additional terms or conditions.
```

Insert badges after the tagline (after line 3):

```md
[![CI](https://github.com/sherazp995/riven/actions/workflows/ci.yml/badge.svg)](https://github.com/sherazp995/riven/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/rustc-1.78+-blue.svg)](https://releases.rs/docs/1.78.0/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
```

### 4.4 `CONTRIBUTING.md` — skeleton

```markdown
# Contributing to Riven

Thanks for considering a contribution! This document covers the practical bits.

## Code of Conduct

By participating in this project, you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Setup

### Prerequisites

- Rust 1.78 or newer (the MSRV; install via [rustup](https://rustup.rs/)).
- A C compiler in your `$PATH` (`cc`). On Debian/Ubuntu: `apt install build-essential`. On macOS: `xcode-select --install`.
- Optional: LLVM 18 for the `--features llvm` backend. Install via `apt install llvm-18-dev` or `brew install llvm@18`.

### Build & test

```bash
git clone https://github.com/sherazp995/riven.git
cd riven
cargo build --workspace
cargo test  --workspace
cargo fmt  --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Project layout

See [README.md](README.md) or `docs/requirements/tier1_00_roadmap.md` for the component breakdown.

## Making a change

1. Open an issue first if your change is more than a trivial fix — this avoids wasted work if the direction isn't right.
2. Fork, create a feature branch from `master`.
3. Write tests for your change. Every new stdlib method, every new compiler pass, every new CLI flag has at least one test.
4. Run the full test suite locally (see "Build & test").
5. Keep commits focused. We do not squash-merge; make your history tell the story.
6. Open a PR. Link the issue. Describe what you changed and why.

## Commit messages

We prefer the [Conventional Commits](https://www.conventionalcommits.org/) style, loosely:

```
type(scope): short summary

Longer explanation, if needed.

Fixes #123
```

`type` is one of `feat`, `fix`, `docs`, `perf`, `refactor`, `test`, `chore`. `scope` is optional and names the subsystem (`lexer`, `parser`, `cli`, ...).

## MSRV policy

The minimum supported Rust version is **1.78**. It is set in the root `Cargo.toml` and enforced by CI. Bumping the MSRV requires a minor version bump.

## What should not be in a PR

- Unrelated whitespace churn.
- Formatter-only changes mixed with semantic changes.
- New dependencies without justification.
- Generated files (anything in `target/`, `Cargo.lock` *is* checked in — see below).
- `LICENSE`-incompatible dependencies (must be compatible with MIT OR Apache-2.0).

### Cargo.lock

We check `Cargo.lock` into version control (it's a binary project, not a library crate consumers depend on).

## Signing off

You do not need a DCO sign-off. By submitting a PR, you agree to license your contribution under MIT OR Apache-2.0, per [README.md](README.md#contribution).

## Where to ask

- File an issue on GitHub for anything that needs discussion.
- Security issues: **do not** open a public issue. See [SECURITY.md](SECURITY.md).
```

### 4.5 `CODE_OF_CONDUCT.md` — Contributor Covenant 2.1

Verbatim from https://www.contributor-covenant.org/version/2/1/code_of_conduct/. Fill in the "Enforcement" section with a contact email:

```markdown
## Enforcement

Instances of abusive, harassing, or otherwise unacceptable behavior may be reported to the community leaders responsible for enforcement at **riven-conduct@example.com** (replace with the actual maintainer email when ready). All complaints will be reviewed and investigated promptly and fairly.
```

### 4.6 `SECURITY.md`

```markdown
# Security Policy

## Supported Versions

Riven is pre-1.0. We support only the latest minor release series. Security fixes land on `master` and are backported to the most recent tagged release.

| Version | Supported          |
|---------|--------------------|
| 0.1.x   | :white_check_mark: |
| < 0.1   | :x:                |

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Email **riven-security@example.com** (replace with the actual address when ready). We aim to acknowledge receipt within 72 hours and provide a remediation plan within 7 days for high-severity issues.

When reporting, include:

- A description of the issue and its impact.
- Steps to reproduce, ideally with a minimal test case.
- The version of Riven affected.
- Your contact info for follow-up.

## Our commitment

- We will confirm the vulnerability and determine its severity.
- We will credit you in the CVE and release notes, unless you request otherwise.
- We will publish a fix before disclosing the vulnerability publicly, if feasible.
- Coordinated disclosure: we prefer a 90-day embargo from report to public disclosure.

## Scope

In scope:

- Miscompilation that causes undefined behavior in safe Riven code.
- Borrow-checker holes that allow data races.
- Linker-line injection via malicious `Riven.toml` (tier 4.01).
- Registry vulnerabilities (tier 4.01).

Out of scope (in the sense of "still important but not handled via this channel"):

- `unsafe` code doing what `unsafe` says it does.
- Denial-of-service by feeding the compiler adversarial inputs. File a regular issue.
- Any behavior of a user-written Riven program.
```

### 4.7 `CHANGELOG.md` — Keep-a-Changelog format

```markdown
# Changelog

All notable changes to Riven are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0: every minor bump may include breaking changes).

## [Unreleased]

### Added
- (entries for work-in-progress land here; moved into the next release section on tag)

## [0.1.0] — 2025-12-XX

### Added
- Initial public release.
- Compiler with lexer, parser, resolver, type inference, borrow checker, MIR lowering, Cranelift backend, LLVM backend (feature-gated).
- `riven` package-manager CLI (new, build, run, check, clean, add, remove, update, tree, verify).
- `rivenc` standalone compiler with `--emit={tokens,ast,hir,mir}` and `fmt`.
- `riven-lsp` server with hover, goto-definition, diagnostics, semantic tokens.
- `riven-repl` with Cranelift JIT.
- VSCode extension.

[Unreleased]: https://github.com/sherazp995/riven/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/sherazp995/riven/releases/tag/v0.1.0
```

### 4.8 `.github/ISSUE_TEMPLATE/bug_report.md`

```markdown
---
name: Bug report
about: Report a miscompilation, panic, or unexpected behavior
labels: bug
---

## What happened

<!-- A clear, concise description of the bug. -->

## What I expected to happen

## Minimal reproducer

```rvn
# smallest .rvn program that triggers it
```

## Versions

- `riven --version`:
- `rivenc --version`:
- OS / arch:
- Rust toolchain (`rustc --version`) if building from source:

## Extra context

<!-- Anything else: stack trace, backtrace via RUST_BACKTRACE=1, related issues. -->
```

### 4.9 `.github/ISSUE_TEMPLATE/feature_request.md`

```markdown
---
name: Feature request
about: Propose a new language feature, stdlib addition, or tooling improvement
labels: enhancement
---

## The problem

<!-- What are you trying to do that you can't today? -->

## Proposed solution

<!-- What do you want the language / tool to do? -->

## Alternatives considered

## Additional context
```

### 4.10 `.github/ISSUE_TEMPLATE/config.yml`

```yaml
blank_issues_enabled: false
contact_links:
  - name: Security vulnerability
    url: mailto:riven-security@example.com
    about: Please report vulnerabilities privately per SECURITY.md.
  - name: Community discussion
    url: https://github.com/sherazp995/riven/discussions
    about: General questions, show-and-tell, design chat.
```

### 4.11 `.github/PULL_REQUEST_TEMPLATE.md`

```markdown
## Summary

<!-- What does this PR do? -->

## Related issue

Fixes #<issue-number>

## Changes

- [ ] <change 1>
- [ ] <change 2>

## Checklist

- [ ] I ran `cargo test --workspace` locally.
- [ ] I ran `cargo fmt --all --check`.
- [ ] I ran `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] I added tests for new behavior.
- [ ] I updated `CHANGELOG.md` under `## [Unreleased]`.
- [ ] I updated documentation if user-facing behavior changed.
```

### 4.12 `CODEOWNERS`

```
# Default owner for everything.
*                           @sherazp995

# Subsystem-specific owners (add as maintainers grow):
# crates/riven-core/        @parser-team
# crates/riven-lsp/         @ide-team
# docs/                     @docs-team
```

### 4.13 Per-crate `Cargo.toml` metadata

For each of `riven-core`, `rivenc`, `riven-cli`, `riven-lsp`, `riven-ide`, `riven-repl`, add to `[package]`:

```toml
description = "<one-line description of this crate's role>"
authors = ["Sheraz <arehman0279@gmail.com>"]
license = "MIT OR Apache-2.0"
repository = "https://github.com/sherazp995/riven"
homepage = "https://github.com/sherazp995/riven"
readme = "../../README.md"
rust-version = "1.78"
```

Once tier 4.01 workspace inheritance lands, these become `*.workspace = true` references.

### 4.14 Root `.gitignore`

```
/target
/fuzz/target
/fuzz/corpus
/fuzz/artifacts
*.o
*.wasm
*.rlib
.DS_Store
.vscode/
.idea/
```

## 5. Architecture / Design

### 5.1 Why MIT OR Apache-2.0?

The Rust ecosystem's default. Rationale:

- **MIT** is maximally permissive. Users embedding Riven in proprietary products have no ambiguity.
- **Apache-2.0** adds an explicit patent grant, protecting downstream users from patent trolls.
- **OR** (not AND) lets users pick. Most pick MIT for simplicity; projects that ship binaries prefer Apache for patent coverage.
- 80%+ of the Rust ecosystem uses this combo (Rust itself, Cargo, tokio, serde, rayon, …). Matching avoids downstream-compatibility surprises.

Alternative considered: **BSD-2-Clause** only. Shorter, simpler. No patent grant. Rejected on the grounds that Riven competes in a space with patent implications (ownership / memory safety research).

Alternative considered: **MPL-2.0**. Weak copyleft. Forces derivatives of MPL files to remain MPL. Not aligned with the ecosystem. Rejected.

Alternative considered: **GPL / AGPL**. Strong copyleft. Makes the compiler useless for proprietary downstream products. Rejected.

### 5.2 Why Contributor Covenant 2.1?

- Industry-standard community CoC.
- Concrete enforcement guidelines (tier-1..tier-4 responses).
- Recognizable to contributors. Don't invent your own CoC.

Alternative: **Django's CoC**. Fine. Less common.

Alternative: **Mozilla's CoC**. Longer, more prescriptive. Fine for Mozilla-sized projects. Overkill for Riven today.

### 5.3 Private security disclosure

Email-based, pre-PGP. Most projects graduate to GitHub's built-in "Security Advisories" (private reports). Recommend: enable that feature and list it in `SECURITY.md` alongside email.

GPG keys: optional in v1; document a PGP fingerprint once we have a stable maintainer group.

### 5.4 Changelog discipline

Keep-a-Changelog format:

- `[Unreleased]` always at top.
- Entries sorted by `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`.
- Link refs at bottom.

PR template includes a "did you update CHANGELOG.md?" checkbox. Reviewers enforce.

Alternative: auto-generated from commit messages (e.g. `git-cliff`). Less churn but noisier. Recommend manual v1.

### 5.5 README badges

Five is a reasonable ceiling. Recommend: CI, MSRV, license. Omit: crates.io version (we don't publish to crates.io), download count (early-stage), lines of code (vanity).

## 6. Implementation Plan — files to touch

### New files

- `LICENSE-MIT`.
- `LICENSE-APACHE`.
- `CONTRIBUTING.md`.
- `CODE_OF_CONDUCT.md`.
- `SECURITY.md`.
- `CHANGELOG.md`.
- `.gitignore` (root).
- `.github/ISSUE_TEMPLATE/bug_report.md`.
- `.github/ISSUE_TEMPLATE/feature_request.md`.
- `.github/ISSUE_TEMPLATE/config.yml`.
- `.github/PULL_REQUEST_TEMPLATE.md`.
- `CODEOWNERS` (or `.github/CODEOWNERS`).

### Touched files

- `README.md:294-296` — replace `TBD` with the dual-license block from §4.3.
- `README.md:3` (after tagline) — badges.
- `crates/*/Cargo.toml` — metadata per §4.13.
- `.github/workflows/release.yml:96` — `cp README.md LICENSE-MIT LICENSE-APACHE "${STAGE}/"` (no wildcard + 2>/dev/null).
- `docs/` README / tutorial pages: link to CONTRIBUTING from where appropriate.

### Tests

Repo-hygiene is mostly prose; tests are meta.

- A CI check that `LICENSE-MIT` and `LICENSE-APACHE` exist at the repo root. (Trivial bash in `ci.yml`: `test -f LICENSE-MIT && test -f LICENSE-APACHE`.)
- A CI check that `CHANGELOG.md` has been modified in any PR touching `src/*` (warning, not error — some PRs legitimately don't need changelog entries).
- `cargo publish --dry-run` as a smoke test on one crate (proves metadata is valid).

## 7. Interactions with Other Tiers

- **Tier 4.01 package manager.** `[package.license = "MIT OR Apache-2.0"]` + `[package.authors = ...]` in the manifest propagates to published packages' registry metadata. `riven publish` rejects `license = "TBD"` or unrecognized SPDX.
- **Tier 4.06 CI.** CI checks that license files exist; `audit.yml` flags license-incompatible transitive deps.
- **Tier 4.07 examples.** Each example carries `license = "MIT OR Apache-2.0"` in its `Riven.toml`. Reusers are clear on terms.
- **Tier 1 derive.** Copyright headers in source files are a style choice. Recommend: **no per-file headers**. The repo's `LICENSE-MIT` + `LICENSE-APACHE` at the root apply to everything under it. Per-file headers are noise in 2024.

## 8. Phasing

This is a single 0.5-week push, best done before anything else in tier 4.

### Phase 8a — ship the files (0.5 week)

1. Commit `LICENSE-MIT`, `LICENSE-APACHE`.
2. Commit `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CHANGELOG.md`.
3. Commit `.github/ISSUE_TEMPLATE/*`, `PULL_REQUEST_TEMPLATE.md`, `CODEOWNERS`.
4. Update `README.md` license section + add badges.
5. Update every `crates/*/Cargo.toml` with metadata.
6. Fix the release workflow's `cp LICENSE*` to not silently succeed.
7. Enable GitHub's built-in Security Advisories in repo settings (one-time toggle).
8. Configure branch protection on `master` (doc 06's requirement).

**Exit:**

- `curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/LICENSE-MIT` returns HTTP 200 with valid MIT text.
- `README.md` says "dual-licensed MIT OR Apache-2.0" and links to both files.
- Releases include both license files.
- A newcomer visiting GitHub sees all six sidebar items (license, CoC, contributing, security, issues, PRs) highlighted.

### Phase 8b — maintenance discipline (ongoing)

1. Every new PR updates `CHANGELOG.md` under `[Unreleased]`.
2. On release tag: move `[Unreleased]` entries to `[x.y.z] - YYYY-MM-DD`, add a new empty `[Unreleased]`.
3. When a new maintainer joins: update `CODEOWNERS`, add a "Maintainers" section to `README.md`.
4. Yearly: bump copyright year in the LICENSE files.

## 9. Open Questions & Risks

1. **Who owns the copyright?** Today: the individual committers. This is fine (matches Rust's model). Document in CONTRIBUTING.md that contributors retain copyright and license their contributions to the project. Alternative: CLA assigning copyright to a single entity. Rejected — adds friction, no clear benefit.
2. **Should we require DCO sign-off?** `git commit -s` appends `Signed-off-by:`. Some projects require it. Recommend: **no**. The LICENSE + CONTRIBUTING.md language is enough. DCO adds friction, catches nothing CLA wouldn't.
3. **Email address for security + CoC.** Both `SECURITY.md` and `CODE_OF_CONDUCT.md` list an email. Recommend: use a dedicated GitHub-project mailbox (Google Workspace for the domain, ~$6/mo), not a personal address. Placeholder is `riven-security@example.com` / `riven-conduct@example.com`; update before landing.
4. **CODEOWNERS granularity.** Everything → `@sherazp995` today. Fine. Split as maintainers grow.
5. **Issue label taxonomy.** `bug`, `enhancement`, `good first issue`, `help wanted`. Out of v1; use GitHub defaults. Don't over-engineer.
6. **Discussions vs Issues.** GitHub Discussions is good for "how do I …" chat. Issues are for tracked bugs/features. Recommend: enable Discussions in repo settings; point `config.yml` at it.
7. **PR template risk: too many checkboxes.** Irritating if PRs require 10 checkboxes. Recommend: keep it to 6. Low-bar boxes reviewers can unambiguously verify.
8. **First-time contributor docs.** Separate from CONTRIBUTING.md? Recommend: no — one file. Add a "Your first PR" section if volume warrants.
9. **License of the docs.** Are `docs/tutorial/*.md` MIT OR Apache-2.0 too? Recommend: yes; the repo-wide LICENSE applies. No separate `docs/LICENSE`.
10. **Third-party assets.** VSCode extension (`editors/vscode/`) may include an icon or README images. Document their provenance in a separate `editors/vscode/THIRD-PARTY-LICENSES.md` if they're not the repo's license.
11. **`.mailmap`.** Maps committer aliases to canonical names. Useful when a contributor commits from multiple machines. Recommend: add when it becomes a problem, not before.
12. **Governance doc.** Who decides when there's a disagreement? Recommend: BDFL (the original author) for v1; spin up a TSC when the project has 5+ regular contributors.

## 10. Acceptance Criteria

- [ ] `LICENSE-MIT` and `LICENSE-APACHE` exist at the repo root, containing the verbatim texts from SPDX.
- [ ] `README.md` has a `## License` section that references both files and explicitly states "dual-licensed under MIT OR Apache-2.0".
- [ ] `README.md` has three badges: CI, MSRV, License.
- [ ] `CONTRIBUTING.md` exists and covers setup, testing, commit style, MSRV, DCO policy (or lack thereof).
- [ ] `CODE_OF_CONDUCT.md` exists with Contributor Covenant 2.1 text and a real contact email.
- [ ] `SECURITY.md` exists with a real private-disclosure address.
- [ ] `CHANGELOG.md` exists with a `## [Unreleased]` section and a `## [0.1.0]` anchor.
- [ ] `.github/ISSUE_TEMPLATE/bug_report.md`, `feature_request.md`, `config.yml` exist.
- [ ] `.github/PULL_REQUEST_TEMPLATE.md` exists.
- [ ] `CODEOWNERS` exists with at least one owner entry.
- [ ] Every `crates/*/Cargo.toml` has `description`, `authors`, `license = "MIT OR Apache-2.0"`, `repository`, `rust-version`.
- [ ] `cargo publish --dry-run -p riven-core` passes the metadata-completeness check.
- [ ] `.github/workflows/release.yml:96` uses an explicit `cp README.md LICENSE-MIT LICENSE-APACHE "${STAGE}/"` (no silent failure).
- [ ] Release tarballs contain `LICENSE-MIT` and `LICENSE-APACHE` at their root.
- [ ] Root `.gitignore` excludes `target/`, `fuzz/target/`, `fuzz/corpus/`, `fuzz/artifacts/`, `.DS_Store`, common editor dirs.
- [ ] GitHub repo has Security Advisories enabled (visible in repo → Security tab).
- [ ] Branch protection on `master` requires CI green (doc 06) and CODEOWNERS review.
- [ ] A random visitor to the GitHub repo sees: license badge in the sidebar, "MIT OR Apache-2.0" text, a highlighted "Community Standards" link showing 100%.
