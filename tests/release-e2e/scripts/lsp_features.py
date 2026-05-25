#!/usr/bin/env python3
"""End-to-end LSP feature tests over stdio JSON-RPC.

Exercises the shipped ruxen-lsp beyond the initialize handshake:
  - did_open → publishDiagnostics
  - did_open with an invalid program → non-empty diagnostics
  - did_save → re-diagnoses after a text change
  - did_close → clears diagnostics
  - hover on a function definition
  - goto_definition on a reference
  - semantic_tokens_full returns a token list

Druxen by the release-e2e harness. Prefers a binary from
$RUXEN_WORKSPACE/target/release/ when set, otherwise falls back to
~/.ruxen/bin/. Exits 0 if every check passes, non-zero otherwise.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time


PROGRAM_OK = """def greet(name: String) -> String
  "Hello, #{name}!"
end

def main
  let msg = greet(String.from("world"))
  puts msg
end
"""

PROGRAM_ERR = """def main
  let x: Int = "nope"
  puts "hi"
end
"""


def frame(payload: dict) -> bytes:
    body = json.dumps(payload).encode("utf-8")
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii")
    return header + body


def read_message(stream) -> dict | None:
    """Read a framed LSP message (blocking). Returns None on EOF."""
    headers: dict[str, str] = {}
    while True:
        line = stream.readline()
        if not line:
            return None
        line = line.decode("ascii", errors="replace").rstrip("\r\n")
        if line == "":
            break
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    length = int(headers.get("content-length", "0"))
    if length == 0:
        return None
    body = stream.read(length)
    if not body or len(body) < length:
        return None
    return json.loads(body.decode("utf-8"))


def collect_until(stream, pred, max_messages: int = 40) -> dict | None:
    """Read up to `max_messages` framed messages; return the first that
    satisfies `pred`. Returns None if none match before the cap."""
    seen = 0
    while seen < max_messages:
        msg = read_message(stream)
        if msg is None:
            return None
        seen += 1
        if pred(msg):
            return msg
    return None


def resolve_binary() -> str | None:
    ws = os.environ.get("RUXEN_WORKSPACE")
    if ws:
        candidate = os.path.join(ws, "target", "release", "ruxen-lsp")
        if os.path.isfile(candidate):
            return candidate
    home = os.path.expanduser("~/.ruxen/bin/ruxen-lsp")
    if os.path.isfile(home):
        return home
    return None


def start_lsp(bin_path: str) -> subprocess.Popen:
    return subprocess.Popen(
        [bin_path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )


def initialize(proc: subprocess.Popen) -> dict:
    msg = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": os.getpid(),
            "rootUri": None,
            "capabilities": {},
            "clientInfo": {"name": "ruxen-e2e-features", "version": "0.0.1"},
        },
    }
    proc.stdin.write(frame(msg))
    proc.stdin.flush()
    resp = read_message(proc.stdout)
    if resp is None:
        raise RuntimeError("no initialize response")
    return resp


def send_notification(proc: subprocess.Popen, method: str, params: dict) -> None:
    msg = {"jsonrpc": "2.0", "method": method, "params": params}
    proc.stdin.write(frame(msg))
    proc.stdin.flush()


def send_request(proc: subprocess.Popen, req_id: int, method: str, params: dict) -> dict | None:
    msg = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}
    proc.stdin.write(frame(msg))
    proc.stdin.flush()
    return collect_until(
        proc.stdout,
        lambda m: m.get("id") == req_id,
    )


def did_open(proc, uri: str, text: str, version: int = 1) -> None:
    send_notification(proc, "textDocument/didOpen", {
        "textDocument": {
            "uri": uri, "languageId": "ruxen",
            "version": version, "text": text,
        },
    })


def did_change_full(proc, uri: str, text: str, version: int) -> None:
    send_notification(proc, "textDocument/didChange", {
        "textDocument": {"uri": uri, "version": version},
        "contentChanges": [{"text": text}],
    })


def did_save(proc, uri: str) -> None:
    send_notification(proc, "textDocument/didSave", {
        "textDocument": {"uri": uri},
    })


def did_close(proc, uri: str) -> None:
    send_notification(proc, "textDocument/didClose", {
        "textDocument": {"uri": uri},
    })


def main() -> int:
    bin_path = resolve_binary()
    if not bin_path:
        print("lsp_features: ruxen-lsp not found", file=sys.stderr)
        return 2

    proc = start_lsp(bin_path)
    failures: list[str] = []

    def check(name: str, cond: bool, detail: str = "") -> None:
        status = "ok" if cond else "FAIL"
        print(f"  [{status}] {name}" + (f" — {detail}" if detail and not cond else ""))
        if not cond:
            failures.append(f"{name}: {detail}")

    try:
        init = initialize(proc)
        caps = init.get("result", {}).get("capabilities", {})
        check("initialize returns capabilities", isinstance(caps, dict) and len(caps) > 0)
        check("has hover_provider", bool(caps.get("hoverProvider")))
        check("has definition_provider", bool(caps.get("definitionProvider")))
        check("has semantic_tokens_provider",
              "semanticTokensProvider" in caps)
        check("has text_document_sync",
              "textDocumentSync" in caps)

        send_notification(proc, "initialized", {})

        # ── Case 1: valid program, expect empty-or-only-warnings diagnostics ─
        uri_ok = "file:///tmp/lsp_feat_ok.rx"
        did_open(proc, uri_ok, PROGRAM_OK)
        diag_ok = collect_until(
            proc.stdout,
            lambda m: m.get("method") == "textDocument/publishDiagnostics"
                      and m.get("params", {}).get("uri") == uri_ok,
        )
        check("did_open(valid) → publishDiagnostics", diag_ok is not None)
        if diag_ok:
            diags = diag_ok.get("params", {}).get("diagnostics", [])
            errors = [d for d in diags if d.get("severity") == 1]
            check("valid program has zero error diagnostics",
                  len(errors) == 0,
                  detail=f"got {len(errors)}: {errors[:2]}")

        # ── Case 2: invalid program, expect >=1 error diagnostic ─────────────
        uri_err = "file:///tmp/lsp_feat_err.rx"
        did_open(proc, uri_err, PROGRAM_ERR)
        diag_err = collect_until(
            proc.stdout,
            lambda m: m.get("method") == "textDocument/publishDiagnostics"
                      and m.get("params", {}).get("uri") == uri_err,
        )
        check("did_open(invalid) → publishDiagnostics", diag_err is not None)
        if diag_err:
            diags = diag_err.get("params", {}).get("diagnostics", [])
            errors = [d for d in diags if d.get("severity") == 1]
            check("invalid program has >=1 error",
                  len(errors) >= 1,
                  detail=f"got {len(errors)} errors, {len(diags)} diagnostics total")

        # ── Case 3: did_change + did_save → re-diagnose (becomes clean) ─────
        did_change_full(proc, uri_err, PROGRAM_OK, version=2)
        did_save(proc, uri_err)
        fixed = collect_until(
            proc.stdout,
            lambda m: m.get("method") == "textDocument/publishDiagnostics"
                      and m.get("params", {}).get("uri") == uri_err,
        )
        check("did_save → republishDiagnostics", fixed is not None)
        if fixed:
            diags = fixed.get("params", {}).get("diagnostics", [])
            errors = [d for d in diags if d.get("severity") == 1]
            check("after did_save with clean source, no errors",
                  len(errors) == 0,
                  detail=f"still had {len(errors)}")

        # ── Case 4: hover request — cover at least one identifier ────────────
        hover_text = PROGRAM_OK
        # Find the position of "greet" in the `greet(` call within main.
        offset = hover_text.index("greet(String.from")
        # Compute (line, col) from offset.
        line = hover_text.count("\n", 0, offset)
        last_nl = hover_text.rfind("\n", 0, offset)
        col = offset - (last_nl + 1)
        hover_params = {
            "textDocument": {"uri": uri_ok},
            "position": {"line": line, "character": col + 1},
        }
        hov = send_request(proc, 10, "textDocument/hover", hover_params)
        check("hover returns a response", hov is not None and "result" in hov)
        # Result may be null or a Hover object; we only check it's reachable
        # (server didn't error out).

        # ── Case 5: goto_definition on greet call site ──────────────────────
        gd_params = dict(hover_params)
        gd = send_request(proc, 11, "textDocument/definition", gd_params)
        check("goto_definition returns a response",
              gd is not None and ("result" in gd))

        # ── Case 6: semantic_tokens_full ────────────────────────────────────
        st = send_request(proc, 12, "textDocument/semanticTokens/full", {
            "textDocument": {"uri": uri_ok},
        })
        check("semantic_tokens_full returns a response",
              st is not None and "result" in st)
        if st and st.get("result"):
            data = st["result"].get("data", []) if isinstance(st["result"], dict) else []
            check("semantic tokens non-empty", len(data) > 0,
                  detail=f"len={len(data)}")

        # ── Case 7: did_close clears diagnostics ────────────────────────────
        did_close(proc, uri_ok)
        closed = collect_until(
            proc.stdout,
            lambda m: m.get("method") == "textDocument/publishDiagnostics"
                      and m.get("params", {}).get("uri") == uri_ok,
        )
        check("did_close → empty diagnostics",
              closed is not None and len(
                  closed.get("params", {}).get("diagnostics", [])
              ) == 0)

        # ── Polite shutdown ─────────────────────────────────────────────────
        shut = send_request(proc, 99, "shutdown", None)
        check("shutdown responds", shut is not None)
        send_notification(proc, "exit", None)
        proc.stdin.close()
        try:
            proc.wait(timeout=2.0)
        except subprocess.TimeoutExpired:
            proc.kill()

    except Exception as e:
        print(f"lsp_features: unhandled error: {e}", file=sys.stderr)
        proc.kill()
        return 5

    if failures:
        print(f"\nlsp_features: {len(failures)} failure(s)", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1
    print("\nlsp_features: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
