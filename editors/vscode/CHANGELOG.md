# Changelog

All notable changes to the Ruxen VS Code extension are documented here. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0]

### Added
- `ruxen.server.args` setting (default `["lsp"]`) so the extension launches the
  language server as the `lsp` subcommand of the `ruxen` binary.
- `package` / `publish` / `publish:openvsx` npm scripts for building and
  releasing the extension.
- README, license pointer, and marketplace metadata (`repository`, `license`,
  `keywords`, `bugs`, `homepage`).
- Extension icon: a "forged ruby" mark (a brilliant-cut gem with a ruby crown
  melting into a molten-rust pavilion over a forge glow) — the Ruby + Rust
  fusion. Generated from `scripts/make_icon.py`.
- esbuild bundling (`esbuild.js`): the published `.vsix` is now a single
  bundled `out/extension.js` (~102 KB / 10 files, down from ~294 KB / 210)
  instead of shipping the full `node_modules` tree.

### Changed
- The default server path now resolves the unified `ruxen` binary
  (`target/release/ruxen`, then `ruxen` on `PATH`) instead of a standalone
  `ruxen-lsp` binary that the current toolchain no longer produces.

### Fixed
- Packaging no longer strips `node_modules`, so the bundled `.vsix` ships its
  runtime dependency (`vscode-languageclient`) and starts correctly.
- Server resolution now checks that a workspace `target/{release,debug}/ruxen`
  build actually exists before using it, and otherwise falls back to `ruxen` on
  `PATH`. Previously, opening any folder without a release build pointed the
  client at a non-existent binary ("couldn't create connection to server").
- Dropped the explicit `TransportKind.stdio`, which made `vscode-languageclient`
  append a `--stdio` flag the `ruxen lsp` subcommand rejects (server exited with
  code 2). stdio is the default transport for an executable server and needs no
  flag.
