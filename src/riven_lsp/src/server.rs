use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use riven_ide::analysis::AnalysisResult;
use riven_ide::semantic_tokens::{TOKEN_MODIFIERS, TOKEN_TYPES};

pub struct RivenLsp {
    client: Client,
    state: Arc<RwLock<ServerState>>,
}

struct ServerState {
    documents: HashMap<Url, DocumentState>,
}

struct DocumentState {
    source: String,
    version: i32,
    analysis: Option<AnalysisResult>,
}

impl RivenLsp {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Arc::new(RwLock::new(ServerState {
                documents: HashMap::new(),
            })),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for RivenLsp {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: TOKEN_TYPES.to_vec(),
                                token_modifiers: TOKEN_MODIFIERS.to_vec(),
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: None,
                            ..Default::default()
                        },
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Riven LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let source = params.text_document.text.clone();
        let version = params.text_document.version;

        let analysis = riven_ide::analysis::analyze(&source);
        let diagnostics = riven_ide::diagnostics::collect_diagnostics(&analysis, &uri);

        {
            let mut state = self.state.write().await;
            state.documents.insert(
                uri.clone(),
                DocumentState {
                    source,
                    version,
                    analysis: Some(analysis),
                },
            );
        }

        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;

        // TextDocumentSyncKind::FULL — the full content is in the first change
        if let Some(change) = params.content_changes.into_iter().next() {
            let mut state = self.state.write().await;
            if let Some(doc) = state.documents.get_mut(&uri) {
                doc.source = change.text;
                doc.version = version;
                // Don't re-analyze on every keystroke in Phase 1.
                // Analysis happens on didSave.
            }
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.clone();

        let (source, version) = {
            let state = self.state.read().await;
            match state.documents.get(&uri) {
                Some(doc) => (doc.source.clone(), doc.version),
                None => return,
            }
        };

        let analysis = riven_ide::analysis::analyze(&source);
        let diagnostics = riven_ide::diagnostics::collect_diagnostics(&analysis, &uri);

        {
            let mut state = self.state.write().await;
            if let Some(doc) = state.documents.get_mut(&uri) {
                doc.analysis = Some(analysis);
            }
        }

        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        {
            let mut state = self.state.write().await;
            state.documents.remove(&uri);
        }
        // Clear diagnostics for closed file
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let state = self.state.read().await;
        let doc = match state.documents.get(&uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };
        let analysis = match &doc.analysis {
            Some(a) => a,
            None => return Ok(None),
        };

        let hover_info = riven_ide::hover::hover_at(analysis, position);

        Ok(hover_info.map(|info| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: info.content,
            }),
            range: Some(info.range),
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let position = params.text_document_position_params.position;

        let state = self.state.read().await;
        let doc = match state.documents.get(&uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };
        let analysis = match &doc.analysis {
            Some(a) => a,
            None => return Ok(None),
        };

        let location = riven_ide::goto_def::goto_definition(analysis, position);

        // Replace placeholder URI with the actual document URI
        Ok(location.map(|mut loc| {
            loc.uri = uri;
            GotoDefinitionResponse::Scalar(loc)
        }))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;

        let state = self.state.read().await;
        let doc = match state.documents.get(&uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };
        let analysis = match &doc.analysis {
            Some(a) => a,
            None => return Ok(None),
        };

        let tokens = riven_ide::semantic_tokens::semantic_tokens(analysis);

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }
}
