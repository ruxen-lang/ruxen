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
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    // Both ctrl-space (no trigger) and post-`.`
                    // requests are accepted. `.` is the only
                    // structural trigger; `:` is reserved for the
                    // (not-yet-implemented) `Self::method` form and
                    // module qualification.
                    trigger_characters: Some(vec![".".to_string()]),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: Default::default(),
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
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

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let trigger = params
            .context
            .as_ref()
            .and_then(|c| c.trigger_character.as_deref())
            .and_then(|s| s.chars().next());

        let state = self.state.read().await;
        let doc = match state.documents.get(&uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };
        let analysis = match &doc.analysis {
            Some(a) => a,
            None => return Ok(None),
        };

        let items = riven_ide::completion::completions(analysis, position, trigger);
        if items.is_empty() {
            return Ok(None);
        }
        Ok(Some(CompletionResponse::Array(items)))
    }

    // === wave1: agent-A signature_help ===
    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        Ok(riven_ide::signature_help::signature_help(
            analysis, position,
        ))
    }

    // === wave1: agent-B document_symbols ===
    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        let symbols = riven_ide::document_symbols::document_symbols(analysis);
        if symbols.is_empty() {
            return Ok(None);
        }
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    // === wave1: agent-B workspace_symbols ===
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query;
        let state = self.state.read().await;
        let docs: Vec<(Url, &AnalysisResult)> = state
            .documents
            .iter()
            .filter_map(|(uri, doc)| doc.analysis.as_ref().map(|a| (uri.clone(), a)))
            .collect();
        let results = riven_ide::workspace_symbols::workspace_symbols(&docs, &query);
        if results.is_empty() {
            return Ok(None);
        }
        Ok(Some(results))
    }

    // === wave1: agent-C inlay_hints ===
    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(Some(Vec::new()));
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(Some(Vec::new()));
        };
        let cfg = riven_ide::inlay_hints::InlayHintConfig::default();
        let hints = riven_ide::inlay_hints::inlay_hints(analysis, range, &cfg);
        Ok(Some(hints))
    }

    // === wave1: agent-D folding_ranges ===
    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        let ranges = riven_ide::folding::folding_ranges(analysis);
        Ok(Some(ranges))
    }

    // === wave1: agent-E document_formatting ===
    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let source = {
            let state = self.state.read().await;
            match state.documents.get(&uri) {
                Some(doc) => doc.source.clone(),
                None => return Ok(None),
            }
        };
        Ok(riven_ide::format::format_document(&source))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let source = {
            let state = self.state.read().await;
            match state.documents.get(&uri) {
                Some(doc) => doc.source.clone(),
                None => return Ok(None),
            }
        };
        Ok(riven_ide::format::format_range(&source, range))
    }

    // === wave1: agent-F type_definition ===
    async fn goto_type_definition(
        &self,
        params: request::GotoTypeDefinitionParams,
    ) -> Result<Option<request::GotoTypeDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let position = params.text_document_position_params.position;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        let Some(mut location) = riven_ide::type_def::type_definition(analysis, position) else {
            return Ok(None);
        };
        location.uri = uri;
        Ok(Some(request::GotoTypeDefinitionResponse::Scalar(location)))
    }

    // === wave1: agent-G code_actions ===
    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri.clone();
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(Some(Vec::new()));
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(Some(Vec::new()));
        };
        let actions =
            riven_ide::code_actions::code_actions(analysis, params.range, &params.context, &uri);
        Ok(Some(actions))
    }

    // === wave2: agent-I references ===
    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let position = params.text_document_position.position;
        let include_decl = params.context.include_declaration;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        let mut locs = riven_ide::references::references(analysis, position, include_decl);
        for loc in locs.iter_mut() {
            loc.uri = uri.clone();
        }
        Ok(Some(locs))
    }

    // === wave2: agent-J document_highlight ===
    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        let highlights = riven_ide::highlight::document_highlights(analysis, position);
        if highlights.is_empty() {
            return Ok(None);
        }
        Ok(Some(highlights))
    }

    // === wave2: agent-K prepare_rename + rename ===
    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        let Some(range) = riven_ide::rename::prepare_rename(analysis, position) else {
            return Ok(None);
        };
        Ok(Some(PrepareRenameResponse::Range(range)))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let position = params.text_document_position.position;
        let new_name = params.new_name;
        let state = self.state.read().await;
        let Some(doc) = state.documents.get(&uri) else {
            return Ok(None);
        };
        let Some(analysis) = &doc.analysis else {
            return Ok(None);
        };
        Ok(riven_ide::rename::rename(
            analysis, &uri, position, &new_name,
        ))
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
