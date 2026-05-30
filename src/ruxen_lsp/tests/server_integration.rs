//! End-to-end integration tests that drive the real `ruxen-lsp` binary over
//! JSON-RPC stdio. Each test spawns the server, performs a handshake, sends
//! requests / notifications, and asserts on the replies.
//!
//! Because the server is analyzed on `didOpen` / `didSave` and diagnostics
//! are published *asynchronously*, tests use the helper
//! `open_and_collect_diagnostics` which waits for the first `publishDiagnostics`
//! notification matching the opened URI (with a wall-clock timeout).

mod common;

use std::time::Duration;

use serde_json::{json, Value};

use common::{LspClient, DEFAULT_TIMEOUT};

// ── 1. initialize + initialized ──────────────────────────────────────────

#[test]
fn initialize_reports_expected_capabilities() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let caps = client.initialize();

    // hover provider
    let hover = caps
        .get("hoverProvider")
        .unwrap_or_else(|| panic!("missing hoverProvider in caps: {}", caps));
    assert!(
        hover.as_bool() == Some(true) || hover.is_object(),
        "hoverProvider must be true or an options object, got: {}",
        hover
    );

    // definition provider
    let definition = caps
        .get("definitionProvider")
        .unwrap_or_else(|| panic!("missing definitionProvider in caps: {}", caps));
    assert!(
        definition.as_bool() == Some(true) || definition.is_object(),
        "definitionProvider must be true or an options object, got: {}",
        definition
    );

    // semantic tokens
    let st = caps
        .get("semanticTokensProvider")
        .unwrap_or_else(|| panic!("missing semanticTokensProvider in caps: {}", caps));
    let legend = st
        .get("legend")
        .unwrap_or_else(|| panic!("semanticTokensProvider missing legend: {}", st));
    let types = legend
        .get("tokenTypes")
        .and_then(|t| t.as_array())
        .expect("tokenTypes array");
    assert!(
        !types.is_empty(),
        "expected at least one tokenType in the legend"
    );
    assert!(
        st.get("full").is_some(),
        "semanticTokensProvider should expose `full` support, got: {}",
        st
    );

    // text document sync
    let sync = caps
        .get("textDocumentSync")
        .unwrap_or_else(|| panic!("missing textDocumentSync: {}", caps));
    // tower-lsp serializes the options form — verify open_close + change
    let sync_opts = sync.as_object().unwrap_or_else(|| {
        panic!(
            "textDocumentSync should be an options object, got: {}",
            sync
        )
    });
    assert_eq!(
        sync_opts.get("openClose"),
        Some(&Value::Bool(true)),
        "openClose should be true, got: {}",
        sync
    );
    // change kind FULL = 1
    assert_eq!(
        sync_opts.get("change").and_then(|c| c.as_i64()),
        Some(1),
        "change should be 1 (Full), got: {}",
        sync
    );

    client.shutdown_and_exit();
}

// ── 2. valid source produces no error diagnostics ───────────────────────

#[test]
fn did_open_valid_source_publishes_no_errors() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let uri = "file:///t/valid.rx";
    let diags = client.open_and_collect_diagnostics(
        uri,
        "def main\n  puts \"hi\"\nend\n",
        1,
        DEFAULT_TIMEOUT,
    );

    // Any diagnostics present must be warnings/hints, never errors.
    let errors: Vec<&Value> = diags
        .iter()
        .filter(|d| d.get("severity").and_then(|s| s.as_i64()) == Some(1))
        .collect();
    assert!(
        errors.is_empty(),
        "expected no error diagnostics for a valid program, got: {:?}",
        errors
    );

    client.shutdown_and_exit();
}

// ── 3. type error diagnostic ─────────────────────────────────────────────

#[test]
fn did_open_type_error_publishes_diagnostic() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let uri = "file:///t/type_error.rx";
    let diags = client.open_and_collect_diagnostics(
        uri,
        "def main\n  let x: Int = \"hi\"\nend\n",
        1,
        DEFAULT_TIMEOUT,
    );

    assert!(
        !diags.is_empty(),
        "expected at least one diagnostic for type mismatch"
    );
    let has_error = diags
        .iter()
        .any(|d| d.get("severity").and_then(|s| s.as_i64()) == Some(1));
    assert!(
        has_error,
        "expected at least one ERROR-severity diagnostic, got: {:?}",
        diags
    );
    // Every diagnostic should have a range.
    for d in &diags {
        assert!(d.get("range").is_some(), "diagnostic missing range: {}", d);
    }

    client.shutdown_and_exit();
}

// ── 4. parse error diagnostic ────────────────────────────────────────────

#[test]
fn did_open_parse_error_publishes_diagnostic() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let uri = "file:///t/parse_error.rx";
    // `def` with no body / identifier is a parse error.
    let diags = client.open_and_collect_diagnostics(uri, "def\n", 1, DEFAULT_TIMEOUT);

    assert!(
        !diags.is_empty(),
        "expected at least one diagnostic for parse error"
    );
    let has_error = diags
        .iter()
        .any(|d| d.get("severity").and_then(|s| s.as_i64()) == Some(1));
    assert!(
        has_error,
        "expected ERROR-severity diagnostic for parse error, got: {:?}",
        diags
    );

    client.shutdown_and_exit();
}

// ── 5. did_change → did_save re-diagnoses ───────────────────────────────

#[test]
fn did_change_then_save_reanalyzes_document() {
    // Server analyses on `didOpen` and re-analyses on `didSave` (see
    // server.rs: the `didChange` handler only stores the new text).
    // So we first open a broken program, then `didChange` it into a valid
    // one, then `didSave` to trigger reanalysis and expect empty diagnostics.
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let uri = "file:///t/changing.rx";
    let broken = "def main\n  let x: Int = \"hi\"\nend\n";
    let fixed = "def main\n  let x: Int = 7\nend\n";

    let diags_before = client.open_and_collect_diagnostics(uri, broken, 1, DEFAULT_TIMEOUT);
    let had_error_before = diags_before
        .iter()
        .any(|d| d.get("severity").and_then(|s| s.as_i64()) == Some(1));
    assert!(
        had_error_before,
        "expected an error diagnostic before change, got: {:?}",
        diags_before
    );

    // Change the document content to the fixed program.
    client
        .send_notification(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": fixed } ],
            }),
        )
        .expect("send didChange");

    // Now save — this triggers re-analysis + publishDiagnostics.
    client
        .send_notification(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri } }),
        )
        .expect("send didSave");

    let diags_after = client.collect_diagnostics_for(uri, DEFAULT_TIMEOUT);
    let errors_after: Vec<&Value> = diags_after
        .iter()
        .filter(|d| d.get("severity").and_then(|s| s.as_i64()) == Some(1))
        .collect();
    assert!(
        errors_after.is_empty(),
        "expected no errors after save of fixed program, got: {:?}",
        errors_after
    );

    client.shutdown_and_exit();
}

// ── 6. hover on a function call ─────────────────────────────────────────

#[test]
fn hover_on_function_call_returns_signature() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    // Layout makes the cursor position trivial to compute:
    // line 0: 'def greet(n: Int) -> String'
    // line 1: '  "hello"'
    // line 2: 'end'
    // line 3: 'def main'
    // line 4: '  let s = greet(5)'   ← cursor inside "greet"
    // line 5: 'end'
    //
    // Binding the call to `let s` keeps `main`'s return type `Unit` and
    // avoids any downstream type mismatches from a stray String tail expr.
    let source =
        "def greet(n: Int) -> String\n  \"hello\"\nend\ndef main\n  let s = greet(5)\nend\n";
    let uri = "file:///t/hover.rx";

    let _ = client.open_and_collect_diagnostics(uri, source, 1, DEFAULT_TIMEOUT);

    // Position the cursor inside "greet" on the call line.
    // "  let s = greet(5)" — "greet" starts at column 10, so column 12 lands
    // inside the identifier (the 'e').
    let id = client
        .send_request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 4, "character": 12 },
            }),
        )
        .expect("send hover");
    let mut bucket = Vec::new();
    let result = client.recv_response_timeout(id, &mut bucket, DEFAULT_TIMEOUT);

    assert!(
        !result.is_null(),
        "expected a hover response, got null (source did not analyze?)"
    );
    let contents = result
        .get("contents")
        .unwrap_or_else(|| panic!("hover missing contents field: {}", result));
    let value = contents
        .get("value")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("hover contents missing value: {}", contents));
    // The function signature renderer emits "def greet(n: Int) -> String".
    assert!(
        value.contains("greet"),
        "hover should mention the function name, got: {:?}",
        value
    );
    assert!(
        value.contains("String") || value.contains("Int") || value.contains("Fn"),
        "hover should contain type info, got: {:?}",
        value
    );

    client.shutdown_and_exit();
}

// ── 7. goto_definition on a function reference ──────────────────────────

#[test]
fn goto_definition_points_at_definition_line() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    // line 0: 'def helper -> Int'   ← definition lives on this line
    // line 1: '  42'
    // line 2: 'end'
    // line 3: 'def main'
    // line 4: '  let v = helper()'  ← cursor inside "helper" reference
    // line 5: 'end'
    //
    // Parens force an explicit call, so the inferred type of `v` is `Int`
    // regardless of any auto-invoke rules for nullary functions — this
    // keeps the program well-typed and the node-finder reliably lands on
    // the call expression.
    let source = "def helper -> Int\n  42\nend\ndef main\n  let v = helper()\nend\n";
    let uri = "file:///t/goto.rx";

    let _ = client.open_and_collect_diagnostics(uri, source, 1, DEFAULT_TIMEOUT);

    // "  let v = helper()" — "helper" starts at column 10; column 12 is
    // inside the identifier.
    let id = client
        .send_request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": 4, "character": 12 },
            }),
        )
        .expect("send definition");
    let mut bucket = Vec::new();
    let result = client.recv_response_timeout(id, &mut bucket, DEFAULT_TIMEOUT);

    assert!(!result.is_null(), "expected a Location response, got null");

    // Response is either a single Location or Location[] — handle both.
    let loc = if result.is_array() {
        result
            .as_array()
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or_else(|| panic!("empty location array: {}", result))
    } else {
        result.clone()
    };

    // URI on the definition must match what we sent.
    assert_eq!(
        loc.get("uri").and_then(|u| u.as_str()),
        Some(uri),
        "definition URI should match opened document: {}",
        loc
    );
    // The definition lives on line 0 (the `def helper ...` line).
    let def_line = loc
        .get("range")
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(|l| l.as_i64())
        .unwrap_or_else(|| panic!("definition range missing start.line: {}", loc));
    assert_eq!(
        def_line, 0,
        "definition should be on line 0, got {} (full response: {})",
        def_line, loc
    );

    client.shutdown_and_exit();
}

// ── 8. semantic tokens returned for a non-trivial program ───────────────

#[test]
fn semantic_tokens_full_returns_nonempty_data() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let source = "def main\n  let x = 42\n  puts \"#{x}\"\nend\n";
    let uri = "file:///t/tokens.rx";
    let _ = client.open_and_collect_diagnostics(uri, source, 1, DEFAULT_TIMEOUT);

    let id = client
        .send_request(
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": uri } }),
        )
        .expect("send semanticTokens/full");
    let mut bucket = Vec::new();
    let result = client.recv_response_timeout(id, &mut bucket, DEFAULT_TIMEOUT);

    assert!(
        !result.is_null(),
        "expected a SemanticTokens response, got null"
    );
    let data = result
        .get("data")
        .and_then(|d| d.as_array())
        .unwrap_or_else(|| panic!("semanticTokens response missing `data` array: {}", result));
    assert!(
        !data.is_empty(),
        "expected at least one token in the delta-encoded array, got empty"
    );
    // Delta encoding produces groups of 5 u32s per token.
    assert_eq!(
        data.len() % 5,
        0,
        "token delta array length {} not a multiple of 5",
        data.len()
    );

    client.shutdown_and_exit();
}

// ── 9. shutdown cleanly ─────────────────────────────────────────────────

#[test]
fn shutdown_terminates_server_cleanly() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let id = client
        .send_request("shutdown", Value::Null)
        .expect("send shutdown");
    let mut bucket = Vec::new();
    let result = client.recv_response_timeout(id, &mut bucket, DEFAULT_TIMEOUT);
    // `shutdown` should return null (or an empty object).
    assert!(
        result.is_null() || result.is_object(),
        "shutdown result should be null or an object, got: {}",
        result
    );

    client
        .send_notification("exit", Value::Null)
        .expect("send exit");

    // After `exit`, the server must close stdout within our timeout. The
    // reader thread returns on EOF. Drain any buffered messages so they
    // don't linger, and rely on Drop to wait-or-kill the child.
    let _leftovers = client.drain_for(Duration::from_millis(300));
}

// ── 10. did_close clears diagnostics ────────────────────────────────────

#[test]
fn did_close_clears_diagnostics() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let uri = "file:///t/closing.rx";
    let diags = client.open_and_collect_diagnostics(
        uri,
        "def main\n  let x: Int = \"nope\"\nend\n",
        1,
        DEFAULT_TIMEOUT,
    );
    assert!(
        !diags.is_empty(),
        "expected diagnostics before close, got none"
    );

    client
        .send_notification(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        )
        .expect("send didClose");

    // After close, the server publishes an empty diagnostics list for the URI.
    let cleared = client.collect_diagnostics_for(uri, DEFAULT_TIMEOUT);
    assert!(
        cleared.is_empty(),
        "expected empty diagnostics after didClose, got: {:?}",
        cleared
    );

    client.shutdown_and_exit();
}

// ── 11. hover on an unopened document returns null ──────────────────────

#[test]
fn hover_on_unknown_document_returns_null() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    // We intentionally do NOT open the document — the server should return
    // a null result (no hover info) rather than error out.
    let id = client
        .send_request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": "file:///t/nope.rx" },
                "position": { "line": 0, "character": 0 },
            }),
        )
        .expect("send hover");
    let mut bucket = Vec::new();
    let result = client.recv_response_timeout(id, &mut bucket, DEFAULT_TIMEOUT);
    assert!(
        result.is_null(),
        "hover on unknown document should be null, got: {}",
        result
    );

    client.shutdown_and_exit();
}

// ── 12. semantic tokens on unopened doc returns null ────────────────────

#[test]
fn semantic_tokens_on_unknown_document_returns_null() {
    let mut client = LspClient::spawn().expect("spawn lsp");
    let _ = client.initialize();

    let id = client
        .send_request(
            "textDocument/semanticTokens/full",
            json!({ "textDocument": { "uri": "file:///t/also-nope.rx" } }),
        )
        .expect("send semanticTokens/full");
    let mut bucket = Vec::new();
    let result = client.recv_response_timeout(id, &mut bucket, DEFAULT_TIMEOUT);
    assert!(
        result.is_null(),
        "semanticTokens on unknown document should be null, got: {}",
        result
    );

    client.shutdown_and_exit();
}
