//! `ruxen_lsp` library entry point.
//!
//! Exposes `run()` so the unified `ruxen` driver can launch the LSP server as
//! `ruxen lsp` without going through a separate binary. There is no standalone
//! `ruxen-lsp` binary anymore — `ruxen_cli` is the only crate with a `[[bin]]`.

mod server;

use tower_lsp::{LspService, Server};

/// Run the LSP server over stdio until the client disconnects.
///
/// Returns `Ok(())` on a clean shutdown; never returns an `Err` today, but
/// the signature matches the rest of the `ruxen` subcommand dispatch so the
/// driver can `?` it uniformly.
pub fn run() -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to start tokio runtime: {}", e))?;

    rt.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let (service, socket) = LspService::new(server::RuxenLsp::new);
        Server::new(stdin, stdout, socket).serve(service).await;
    });

    Ok(())
}
