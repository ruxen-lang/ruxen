use tower_lsp::{LspService, Server};

mod server;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--version" | "-V" => {
                println!("riven-lsp {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                println!("riven-lsp {}", env!("CARGO_PKG_VERSION"));
                println!();
                println!("Language Server Protocol server for Riven.");
                println!("Communicates over stdin/stdout; launch from your editor.");
                return;
            }
            _ => {}
        }
    }

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(server::RivenLsp::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
