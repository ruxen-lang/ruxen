import * as fs from "fs";
import * as path from "path";
import { workspace, ExtensionContext, window } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: ExtensionContext): Promise<void> {
  const config = workspace.getConfiguration("ruxen");
  const configured = config.get<string>("server.path")?.trim();

  // The Ruxen language server is launched as the `lsp` subcommand of the
  // unified `ruxen` binary (i.e. `ruxen lsp`), so the default arguments are
  // `["lsp"]`. Power users can override them via `ruxen.server.args`.
  const args = config.get<string[]>("server.args") ?? ["lsp"];

  const command = configured && configured.length > 0
    ? configured
    : defaultServerPath(context);

  // Communicate over stdio. For an `Executable` server, stdio is the default
  // transport and — unlike specifying `TransportKind.stdio` explicitly — does
  // NOT append a `--stdio` argument, which `ruxen lsp` does not accept.
  const serverOptions: ServerOptions = {
    run: { command, args },
    debug: { command, args },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "ruxen" },
      { scheme: "untitled", language: "ruxen" },
    ],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.{ruxen,rx}"),
    },
  };

  client = new LanguageClient("ruxen", "Ruxen LSP", serverOptions, clientOptions);

  try {
    await client.start();
  } catch (err) {
    window.showErrorMessage(
      `Failed to start the Ruxen language server (${command} ${args.join(" ")}). ` +
        `Set 'ruxen.server.path' to your 'ruxen' binary in settings. ${err}`,
    );
  }
}

export async function deactivate(): Promise<void> {
  if (client) {
    await client.stop();
    client = undefined;
  }
}

function defaultServerPath(_context: ExtensionContext): string {
  // The language server ships inside the unified `ruxen` binary, launched as
  // `ruxen lsp`. Prefer a build that exists inside an open workspace folder
  // (release first, then debug); otherwise fall back to `ruxen` on PATH so the
  // extension works regardless of which folder is open.
  const ext = process.platform === "win32" ? ".exe" : "";
  const name = `ruxen${ext}`;
  for (const folder of workspace.workspaceFolders ?? []) {
    // fsPath is only meaningful for local files; skip remote/virtual roots.
    if (folder.uri.scheme !== "file") {
      continue;
    }
    for (const profile of ["release", "debug"]) {
      const candidate = path.join(folder.uri.fsPath, "target", profile, name);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  // Resolved against PATH by the OS when spawned.
  return name;
}
