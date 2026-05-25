import * as path from "path";
import { workspace, ExtensionContext, window } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: ExtensionContext): Promise<void> {
  const config = workspace.getConfiguration("ruxen");
  const configured = config.get<string>("server.path")?.trim();

  const command = configured && configured.length > 0
    ? configured
    : defaultServerPath(context);

  const serverOptions: ServerOptions = {
    run: { command, transport: TransportKind.stdio },
    debug: { command, transport: TransportKind.stdio },
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
      `Failed to start ruxen-lsp (${command}). Set 'ruxen.server.path' in settings. ${err}`,
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
  const ext = process.platform === "win32" ? ".exe" : "";
  const ws = workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (ws) {
    return path.join(ws, "target", "release", `ruxen-lsp${ext}`);
  }
  return `ruxen-lsp${ext}`;
}
