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
- Add tests for new behavior. The compiler has integration suites under
  `crates/*/tests/` and `tests/release-e2e/`.
- Run `cargo test --workspace` locally before pushing. CI will run the
  same plus an MSRV check (Rust 1.91).
- `cargo fmt --all` and `cargo clippy --workspace` are advisory today
  while we clear backlog. Fixing lint drift in the area you touch is
  appreciated; large lint-only PRs should be discussed first.

## Commit messages

Conventional Commits style is preferred but not enforced. The body
should explain *why*, not just *what*.

```
fix(mir): close P0.2 Drop gaps for reassignment and loop bodies
```

## Error codes

Every diagnostic emitted via `Diagnostic::error_with_code` must have
an entry in `crates/ruxen-core/src/diagnostics/codes.rs`. The
`tests/error_code_registry.rs` integration test will fail the build
if a code is used without a registry entry.

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
scripts/ruxenc-rss-cap.sh -- <args>
```

It SIGKILLs the child if RSS exceeds 8 GiB.

## License

By submitting a PR you agree to license your contribution under the
terms of both LICENSE-MIT and LICENSE-APACHE (dual-licensed).
