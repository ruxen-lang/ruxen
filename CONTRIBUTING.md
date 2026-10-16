# Contributing to Ruxen

Thanks for your interest. Ruxen is pre-1.0 — APIs and internals can
shift between commits.

## Getting started

1. Install Rust 1.91+ (workspace MSRV).
2. Clone the repo.
3. Run `cargo build` and `cargo test --workspace`.
4. Read [`docs/requirements/ROADMAP.md`](docs/requirements/ROADMAP.md)
   for the active roadmap.

## Workflow

- Open an issue before starting non-trivial work so we can confirm the
  approach. For small fixes (typos, obvious bugs, docs) just send a PR.
- Branch from `master`. Keep PRs focused — one logical change per PR.
- Add tests for new behavior. See [Testing](#testing) for where each
  kind of test goes.
- While editing, run only the tests your change needs:
  `scripts/affected-tests.sh` maps the changed paths to them. Run
  `cargo test --workspace` once before pushing.
- `cargo fmt --all` and `cargo clippy --workspace` are advisory today
  while we clear backlog. Fixing lint drift in the area you touch is
  appreciated; large lint-only PRs should be discussed first.

## Testing

Where tests live:

- **Unit tests** sit next to the code (`#[cfg(test)]` modules).
- **Integration tests** are one binary per crate,
  `<crate>/tests/it/main.rs`, with one module per file. Run one module
  with `cargo test -p <crate> --test it -- <module>::`.
  `release_e2e_smoke` stays its own binary because CI calls it by name.
- **Stdlib behaviour** is tested in Ruxen, in
  `library/std/<pkg>/tests/*.rx` (the Tester DSL, see
  `docs/tutorial/19-writing-and-running-tests.md`). Build with
  `cargo build --release --bins`, then run `scripts/test-stdlib.sh
  [pkg ...]`.
- **End-to-end fixtures** are `tests/release-e2e/cases/NNN_<topic>.rx`
  with the expected stdout in `expected/NNN_<topic>.out`. Run some with
  `RUXEN_E2E_CASES=NNN_a,NNN_b cargo test -p ruxen_core --test
  release_e2e_smoke -- --ignored`, or all of them by leaving the
  variable unset.

`tests/release-e2e/run.sh` drives the built `ruxen` binary
(`RUXEN_WORKSPACE=$PWD` picks `target/release`). `PHASES` picks what
runs; the default skips the REPL fixture replay, which covers only the
cases in `tests/release-e2e/repl-cases.txt`. The `parity` phase formats
one fixture with the binary; `syntax_parity` covers the whole corpus.

CI has two lanes:

- **Fast lane** (`ci.yml`, every push): `cargo test --workspace`, a
  filtered `release_e2e_smoke`, `scripts/test-stdlib.sh`, an MSRV
  build, and advisory fmt and clippy.
- **Slow lane** (`pr-ready.yml`, on master and on PRs that are not
  drafts): the full `release_e2e_smoke` on Linux and macOS, the `llvm`
  feature, the `run.sh` binaries, cli and lsp phases, and the cross
  build check.

## Commit messages

Conventional Commits style is preferred but not enforced. The body
should explain *why*, not just *what*.

```
fix(mir): close P0.2 Drop gaps for reassignment and loop bodies
```

## Error codes

Every diagnostic emitted via `Diagnostic::error_with_code` must have
an entry in `compiler/ruxen_core/src/diagnostics/codes.rs` and a
`docs/errors/<code>.md` page (`ruxen explain` picks it up). The
`error_code_registry` test (`compiler/ruxen_core/tests/it/`)
checks that every registered code has a doc and every doc a registry
entry.

Reserved namespaces (see `docs/requirements/ROADMAP.md`):

| Range         | Owner                       |
|---------------|-----------------------------|
| E0001-E0099   | lexer                       |
| E0601-E0609   | auto-synth mixins           |
| E0700-E0799   | tier-2 type system          |
| E1001-E1099   | borrow / mixin / include    |

## Memory caps

`ruxenc` can leak memory on certain inputs. Use the wrapper script when
running it on large or untrusted sources:

```
RUXENC_BIN=target/debug/ruxen scripts/ruxenc-rss-cap.sh compile <file.rx>
```

It SIGKILLs the child if RSS exceeds 8 GiB.

## License

By submitting a PR you agree to license your contribution under the
terms of both LICENSE-MIT and LICENSE-APACHE (dual-licensed).
