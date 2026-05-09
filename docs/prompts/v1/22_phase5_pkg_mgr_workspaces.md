# 22 — Phase 5: package manager extensions (T4.01)

**Depends on:** Phase 2 stdlib stable.
**Reads:** `docs/requirements/tier4_01_package_manager.md`.

## Scope

Workspaces + publish/registry are the v1 deliverables. Per
TEC-13, registry is **git-URL only** (Go-style).

## A. Workspaces

```toml
# top-level Riven.toml
[workspace]
members = ["pkg-a", "pkg-b", "examples/*"]
```

### TDD
- Unit test: `Manifest::is_workspace_root` detects workspace.
- E2E: build a workspace with two members + one path-dep between
  them.
- Test that `riven build` from any member finds workspace root.

### Implementation
- `[workspace]` section parsing.
- Resolve dependencies workspace-wide so members can reference each
  other by name without redundant `path = ...`.
- `target/` is shared at workspace root.

## B. Publish (git-URL registry)

`riven publish` packages the current crate and pushes a tag to a
configured git remote. Consumers reference by `git = "..."`,
`tag = "..."` (already supported per existing `gh` flow).

### TDD
- Integration test: `riven publish --dry-run` on a fixture verifies
  the tarball and tag-name without actually pushing.
- E2E: in a tempdir, `riven publish` to a local bare repo; another
  project consumes via `git = "file:///..."`.

### Implementation
- New subcommand `publish` in CLI.
- Tarball via `tar` shell-out (avoid pulling in tar crate).
- Tag format: `v<package-name>-<version>`.
- Refuses to publish a tagged version that already exists.

## C. Lockfile improvements

- `riven update` already exists. Add `--precise <pkg>=<rev>`.
- `Riven.lock` checksum verification (already present; audit).

## Reserved error codes

- E1600 — workspace member not found
- E1601 — circular path dependency
- E1602 — published version already exists at remote

## Definition of done

- [ ] Workspaces work with multiple members + intra-deps.
- [ ] `riven publish --dry-run` and live publish to local bare repo
      work.
- [ ] CHANGELOG bullet.
