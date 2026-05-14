# Universal rules — read before every prompt

These rules apply to **every** prompt in `docs/prompts/`. Each prompt
inherits them silently. Violations regress the project.

---

## 1. TDD discipline (red → green → refactor)

For every behavior you implement:

1. **Write the failing test first.** Test must reference the symbol /
   surface you intend to ship. Run it; confirm it fails for the
   reason you expect (missing symbol, wrong output, etc.). A test
   that fails with `unrelated error` is not yet a red test — fix
   the harness first.
2. **Implement minimum code to pass.** No extra abstractions, no
   speculative generics, no helpers without a second caller.
3. **Run `cargo test --workspace`.** Every test in the workspace must
   pass. No `#[ignore]`. No `cfg(skip)`. No "fix this later".
4. **Refactor only with green tests.** If a refactor breaks any test,
   revert and try again.
5. **Commit per logical green step.** Commit messages follow the
   template in `CONTRIBUTING.md`.

A prompt is incomplete until the **entire workspace** passes
`cargo test --workspace`, not just the new tests.

## 2. Memory cap

`rivenc` and any process-spawning helper must respect the 8 GiB cap.
Use `scripts/rivenc-rss-cap.sh -- <args>` when invoking the compiler
on tests that exercise large inputs. If a test allocates more than
8 GiB you have a leak; fix it instead of raising the cap.

## 3. No shortcut anti-patterns

The following lead to silent regressions and are forbidden:

- **`riven_noop_passthrough` fallbacks** for unknown methods (P0.5
  lesson). If a method is not implemented, codegen must `Err()`.
- **Mocking the SymbolTable / HIR** in tests. Drive everything from
  `.rvn` fixtures or call into the real `Lowerer`/`Parser`/`Lexer`.
  See `crates/riven-core/tests/drop_fixtures.rs` for the canonical
  pattern.
- **`free(NULL)`-style "safe stub"** for unimplemented runtime fns.
  If `riven_xxx` is declared, it must do its job or panic.
- **Rust-side `#[allow(dead_code)]`** to hide unused error paths in
  the compiler. Use `unimplemented!("...")` so it shows up in
  failures.
- **String-typed magic constants.** New error codes go in
  `crates/riven-core/src/diagnostics/codes.rs`. New runtime function
  names go in the dispatch table at `codegen/runtime.rs`.

## 4. Cross-tier contract

When you change a tier-1 surface (stdlib, drop, derive), check the
test suites in:

- `crates/riven-core/tests/`
- `crates/riven-cli/tests/`
- `tests/release-e2e/cases/`

If your change makes any of these fail, fix the failure as part of
the same PR. Do not skip or `#[ignore]`.

## 5. Error-code namespace discipline

| Range         | Owner                              |
|---------------|------------------------------------|
| E0001-E0099   | lexer                              |
| E0601-E0609   | derive generators (T1.05)          |
| E0700-E0799   | tier-2 type system                 |
| E1001-E1099   | borrow / mixin / include / extension |
| E1100-E1199   | async + concurrency (T1.02/3)      |
| E1200-E1299   | LSP / DX (T3.01)                   |

Pick a code in the right range. Add it to
`diagnostics/codes.rs::REGISTRY` AND emit it via
`Diagnostic::error_with_code`. The
`tests/error_code_registry.rs` test fails the build if you forget.

## 6. CHANGELOG hygiene

Every prompt's PR appends one bullet to `CHANGELOG.md` under
`## [Unreleased]`. Keep-a-Changelog format. One bullet per
user-visible change.

## 7. Doc comment discipline

Every public function, struct, enum, mixin you add gets at least one
`##` doc comment line summarising what it does. P0.13 wired
`##` capture; T3.04 (rivendoc) will harvest it.

## 8. Fixture coverage

For every language-surface feature (parser/lowering/codegen):

- Add at least one positive `.rvn` fixture under
  `tests/release-e2e/cases/NNN_<topic>.rvn` with a matching
  `expected/NNN_<topic>.out`.
- Add at least one negative test (compile error with right code) in
  the relevant unit test module.

## 9. Definition of done — universal

Before a prompt is considered complete:

- [ ] All red tests added; all are now green.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo build --workspace --all-targets` passes.
- [ ] No `#[ignore]` added.
- [ ] No new `riven_noop_passthrough` introductions.
- [ ] CHANGELOG `[Unreleased]` updated with one bullet.
- [ ] CI green on the PR (build + test on Ubuntu and macOS, MSRV).
- [ ] All new public surface (public-by-default; no Ruby visibility
      marker downgrade) has a `##` doc comment.
- [ ] Every new error code is in `diagnostics/codes.rs::REGISTRY`.
