# Ruxen for Visual Studio Code

Language support for [Ruxen](https://github.com/ruxen-lang/ruxen) — a compiled,
statically-typed language that fuses Ruby's expressiveness with Rust's
ownership-based memory safety.

## Features

- Syntax highlighting and language configuration for `.rx` files.
- Language Server Protocol client: diagnostics, completion, hover, and
  go-to-definition, powered by the Ruxen language server (`ruxen lsp`).

## Requirements

The extension needs the `ruxen` binary, which hosts the language server via its
`lsp` subcommand. Build it from the repository root:

```bash
cargo build --release --bin ruxen
```

By default the extension looks for `target/release/ruxen` in the open workspace,
then falls back to `ruxen` on your `PATH`. To use a binary elsewhere, set
`ruxen.server.path` (see below).

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `ruxen.server.path` | `""` | Absolute path to the `ruxen` executable. Empty = `target/release/ruxen` in the workspace, then `ruxen` on `PATH`. |
| `ruxen.server.args` | `["lsp"]` | Arguments passed to the executable. The server is the `lsp` subcommand of `ruxen`. |
| `ruxen.trace.server` | `"off"` | Trace LSP traffic (`off` \| `messages` \| `verbose`). |

## Development

```bash
npm install
npm run compile      # tsc -> out/extension.js
```

Press <kbd>F5</kbd> in this folder to launch an Extension Development Host.

To produce a `.vsix`:

```bash
npm run package
```

## License

Dual-licensed under either of MIT or Apache-2.0, at your option. See
[LICENSE.md](./LICENSE.md).
