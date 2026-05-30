# Agent prelude — read this before every spawned task

This file is appended to every coder/tester/architect agent spawn for
the Ruxen v1 orchestration. It sets up shared invariants and
conventions every agent must follow.

## Before you start: read prior work

Before touching code:

1. Read the prompt file you've been assigned in `docs/prompts/v1/`.
2. Skim recent CHANGELOG entries (`CHANGELOG.md`) for context on what
   already shipped.
3. `git log --oneline -20` for the latest commits.
4. Read the universal rules in `docs/prompts/00_universal_rules.md`.

## Hard rules from Lead

- **No `git push`. No `gh pr create`. No upstream changes.** Local
  commits only. User's hard rule (set 2026-05-05).
- **No `git add -A`** — `tmp/`, scratch files, and any session
  artifacts must NEVER be committed. Stage files explicitly:
  `git add <list of paths>`.
- **No `#[ignore]`, no `ruxen_noop_passthrough`, no mocked
  HIR/SymbolTable, no `#[allow(dead_code)]` shortcuts** (universal
  rules §3-9 in `docs/prompts/00_universal_rules.md`).
- **Every error code emitted must be in
  `crates/ruxen-core/src/diagnostics/codes.rs::REGISTRY` AND have a
  `docs/errors/<code>.md` long-form explainer**. The registry-coverage
  test at `crates/ruxen-cli/tests/explain_long_form.rs` will fail the
  build otherwise.
- **8 GiB RSS cap** on `ruxenc`. Wrap heavy invocations:
  `scripts/ruxenc-rss-cap.sh -- <args>`.

## Commit flow (since agents can't run `git commit` due to harness)

1. Implement until `cargo test --workspace` is green.
2. Stage your own files only: `git add <specific paths>`.
3. Write commit message draft to `tmp/commit_msg_<sub>.txt` using:
   ```
   <type>(<scope>): <subject> (<#prompt-subitem>)

   <body>
   ```
4. Final reply to Lead: `<sub-id> ready: <list of changed files>`.
5. Lead executes `git commit -F tmp/commit_msg_<sub>.txt`.

## After you succeed: report what worked

When your task lands green, include in your final report a short
section flagging anything reusable for the next agent:

```
PATTERN-WORTH-NOTING: <one-line key> — <one-paragraph value with
file:line citations and the why-this-works rationale>
```

The Lead will surface these in commit messages or CHANGELOG so future
agents inherit them.

## What's worth flagging

- A non-obvious workaround for a tooling/harness quirk (like the
  `_ORIG_FREE` sentinel for the leak-tracker rewriter).
- A reusable code-shape (synth pattern, registration triple, leak
  harness extension).
- A constraint that caused you to backtrack (so the next agent
  doesn't repeat your wasted work).
- A namespace assignment (error code, fixture number, runtime fn name).

NOT worth flagging: project-state snapshots ("commit X has Y"),
trivial syntax, anything already in CLAUDE.md or universal rules.

## Where state lives

- **Repo files** (`docs/prompts/v1/`, `CHANGELOG.md`, `tmp/`): the
  durable plan and DoD checkboxes.
- **Git history**: authoritative for "what changed and why".
- **Lead memory**: ephemeral session context. Agents don't write to
  it directly — surface findings in your final report and Lead will
  decide what's worth keeping.
