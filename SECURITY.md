# Security policy

## Supported versions

Riven is pre-1.0. Only the latest commit on `master` is supported. We
do not backport fixes to older tags.

## Reporting a vulnerability

Please report security issues privately:

- Open a [GitHub Security Advisory](https://github.com/sherazp995/riven/security/advisories/new)
  on the repo, or
- Email the maintainer (see `Cargo.toml` for the current maintainer
  address).

Please **do not** open a public issue for vulnerabilities until the
fix has shipped. We will acknowledge receipt within 7 days and aim
to either ship a fix or publish a public disclosure within 90 days.

## Scope

In scope:

- Memory-safety bugs in `rivenc`, the standard library runtime
  (`crates/riven-core/runtime/runtime.c`), and the package manager.
- Sandbox escapes in `riven-repl`.
- Dependency-resolution attacks (lockfile bypass, registry deps —
  currently deferred per TEC-13).

Out of scope:

- Type-checker false negatives / false positives that do not lead to
  unsoundness or memory unsafety. File these as regular issues.
- Crashes on intentionally malformed input that the compiler rejects
  with a diagnostic.
- DoS via large inputs (compile-bomb-style). Use the
  `scripts/rivenc-rss-cap.sh` wrapper for now.
