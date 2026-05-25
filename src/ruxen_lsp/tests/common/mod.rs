//! Shared test helpers for driving the `ruxen_lsp` server over JSON-RPC stdio.
//!
//! The harness spawns the server binary, frames requests per LSP
//! (`Content-Length: N\r\n\r\n<body>`), and reads framed replies back off
//! stdout. A background reader thread keeps notifications (e.g.
//! `textDocument/publishDiagnostics`) flowing so they don't stall behind
//! unread server output.
//!
//! Every helper imposes a wall-clock timeout so that a hung server fails the
//! test instead of hanging CI.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Default per-operation timeout. Tests that need more can override on a
/// per-call basis via `recv_response_timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Incoming message from the server: either a response to a request we sent
/// (tagged by id) or a server-initiated notification / request.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// JSON-RPC response with a `result` field.
    Response { id: i64, result: Value },
    /// JSON-RPC error response.
    Error { id: i64, error: Value },
    /// Server-initiated notification — e.g. `textDocument/publishDiagnostics`.
    Notification { method: String, params: Value },
    /// Server-initiated request (we generally ignore these in tests).
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

/// Path to the built `ruxen-lsp` binary. Uses `CARGO_BIN_EXE_ruxen-lsp` which
/// cargo injects at test-compile time and already points at the correct
/// profile (release when tests are built with `--release`).
pub fn lsp_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ruxen-lsp"))
}

/// Handle to a running `ruxen_lsp` child process with framed IO plumbing.
///
/// Drop or explicit `shutdown_and_exit` cleans up; the background reader
/// thread joins when the child's stdout closes.
pub struct LspClient {
    child: Option<Child>,
    stdin: ChildStdin,
    rx: Receiver<Incoming>,
    reader: Option<JoinHandle<()>>,
    next_id: i64,
}

impl LspClient {
    pub fn spawn() -> std::io::Result<Self> {
        let bin = lsp_binary();
        assert!(
            bin.is_file(),
            "ruxen-lsp binary not found at {:?} — run `cargo build --release -p ruxen-lsp` first",
            bin
        );

        let mut child = Command::new(&bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");

        // Swallow stderr in a thread so the pipe buffer never fills up and
        // backpressures the server.
        if let Some(mut stderr) = child.stderr.take() {
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                }
            });
        }

        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || reader_loop(stdout, tx));

        Ok(Self {
            child: Some(child),
            stdin,
            rx,
            reader: Some(reader),
            next_id: 1,
        })
    }

    /// Allocate the next request id.
    pub fn next_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Send a JSON-RPC request and return its id.
    ///
    /// Pass `Value::Null` for `params` to omit the field entirely (some
    /// tower-lsp handlers — notably `shutdown` — reject an explicit
    /// `"params": null` with `Unexpected params: null`).
    pub fn send_request(&mut self, method: &str, params: Value) -> std::io::Result<i64> {
        let id = self.next_id();
        let mut msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });
        if !params.is_null() {
            msg["params"] = params;
        }
        write_message(&mut self.stdin, &msg)?;
        Ok(id)
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    pub fn send_notification(&mut self, method: &str, params: Value) -> std::io::Result<()> {
        let mut msg = json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if !params.is_null() {
            msg["params"] = params;
        }
        write_message(&mut self.stdin, &msg)
    }

    /// Wait for the next incoming message, with timeout.
    pub fn recv_any(&self, timeout: Duration) -> Result<Incoming, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    /// Wait for a response or error matching the given id, discarding
    /// unrelated notifications into `bucket`.
    pub fn recv_response_timeout(
        &self,
        id: i64,
        bucket: &mut Vec<Incoming>,
        timeout: Duration,
    ) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for response id={}", id);
            }
            match self.rx.recv_timeout(remaining) {
                Ok(Incoming::Response { id: rid, result }) if rid == id => return result,
                Ok(Incoming::Error { id: rid, error }) if rid == id => {
                    panic!("server returned error for id={}: {}", id, error)
                }
                Ok(other) => bucket.push(other),
                Err(_) => panic!("timed out waiting for response id={}", id),
            }
        }
    }

    /// Drain all messages that arrive within `timeout`, returning them.
    /// Useful for collecting notifications like `publishDiagnostics`.
    pub fn drain_for(&self, timeout: Duration) -> Vec<Incoming> {
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return out;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => out.push(msg),
                Err(RecvTimeoutError::Timeout) => return out,
                Err(RecvTimeoutError::Disconnected) => return out,
            }
        }
    }

    /// Drive the `initialize` + `initialized` handshake and return the
    /// server-reported capabilities object.
    pub fn initialize(&mut self) -> Value {
        let init_params = json!({
            "processId": std::process::id(),
            "rootUri": null,
            "capabilities": {},
            "clientInfo": { "name": "ruxen-lsp-it", "version": "0.1" },
        });
        let id = self
            .send_request("initialize", init_params)
            .expect("send initialize");
        let mut pending = Vec::new();
        let result = self.recv_response_timeout(id, &mut pending, DEFAULT_TIMEOUT);
        self.send_notification("initialized", json!({}))
            .expect("send initialized");
        result
            .get("capabilities")
            .cloned()
            .unwrap_or_else(|| panic!("initialize response missing capabilities: {}", result))
    }

    /// Open a text document and wait up to `timeout` for the first
    /// `publishDiagnostics` for its URI.
    pub fn open_and_collect_diagnostics(
        &mut self,
        uri: &str,
        text: &str,
        version: i32,
        timeout: Duration,
    ) -> Vec<Value> {
        self.send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "ruxen",
                    "version": version,
                    "text": text,
                }
            }),
        )
        .expect("send didOpen");
        self.collect_diagnostics_for(uri, timeout)
    }

    /// Consume messages until the first `publishDiagnostics` for `uri`
    /// arrives, returning its `diagnostics` array. Unrelated messages are
    /// discarded.
    pub fn collect_diagnostics_for(&self, uri: &str, timeout: Duration) -> Vec<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for diagnostics on {}", uri);
            }
            match self.rx.recv_timeout(remaining) {
                Ok(Incoming::Notification { method, params })
                    if method == "textDocument/publishDiagnostics" =>
                {
                    if params.get("uri").and_then(|u| u.as_str()) == Some(uri) {
                        return params
                            .get("diagnostics")
                            .and_then(|d| d.as_array())
                            .cloned()
                            .unwrap_or_default();
                    }
                }
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for diagnostics on {}", uri),
            }
        }
    }

    /// Send a polite `shutdown` + `exit` and wait for the child to exit.
    pub fn shutdown_and_exit(&mut self) {
        let id = match self.send_request("shutdown", Value::Null) {
            Ok(id) => id,
            Err(_) => return,
        };
        // Drain up to 3 s waiting for the shutdown response, discarding
        // everything else. We catch unwind so a timeout doesn't poison the
        // drop path.
        let mut pending = Vec::new();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.recv_response_timeout(id, &mut pending, Duration::from_secs(3));
        }));
        let _ = self.send_notification("exit", Value::Null);
        // Wait with a deadline so a misbehaving server fails the test
        // instead of hanging CI forever.
        if let Some(mut child) = self.child.take() {
            wait_with_timeout(&mut child, Duration::from_secs(3));
        }
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = self.send_notification("exit", Value::Null);
            wait_with_timeout(&mut child, Duration::from_millis(500));
        }
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

/// Wait for `child` to exit, or kill it after `deadline`.
fn wait_with_timeout(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                return;
            }
        }
    }
}

/// Write a framed JSON-RPC message to the server's stdin.
fn write_message<W: Write>(w: &mut W, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg).expect("serialize");
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read framed JSON-RPC messages from the server's stdout and push them
/// onto the channel. Exits when stdout closes or an unparseable frame
/// arrives.
fn reader_loop<R: Read>(reader: R, tx: Sender<Incoming>) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => return, // EOF
                Ok(_) => {}
                Err(_) => return,
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = rest.trim().parse().ok();
            }
            // Ignore other headers (Content-Type etc).
        }
        let length = match content_length {
            Some(n) => n,
            None => return, // malformed
        };
        let mut buf = vec![0u8; length];
        if reader.read_exact(&mut buf).is_err() {
            return;
        }
        let msg: Value = match serde_json::from_slice(&buf) {
            Ok(v) => v,
            Err(_) => return,
        };
        let incoming = parse_incoming(msg);
        if tx.send(incoming).is_err() {
            return;
        }
    }
}

fn parse_incoming(msg: Value) -> Incoming {
    let id = msg.get("id").cloned();
    let method = msg
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_owned);
    let result = msg.get("result").cloned();
    let error = msg.get("error").cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    match (id, method, result, error) {
        (Some(id_val), None, Some(result), _) => {
            let id = id_val.as_i64().unwrap_or(-1);
            Incoming::Response { id, result }
        }
        (Some(id_val), None, _, Some(error)) => {
            let id = id_val.as_i64().unwrap_or(-1);
            Incoming::Error { id, error }
        }
        (Some(id_val), Some(method), _, _) => Incoming::Request {
            id: id_val,
            method,
            params,
        },
        (None, Some(method), _, _) => Incoming::Notification { method, params },
        _ => Incoming::Notification {
            method: String::new(),
            params: Value::Null,
        },
    }
}
