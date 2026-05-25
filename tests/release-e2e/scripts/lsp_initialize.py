#!/usr/bin/env python3
"""Send an LSP `initialize` request to ruxen-lsp over stdio and verify
the server returns a well-formed JSON-RPC response with a `capabilities`
field.

Exits 0 on success, non-zero with a human-readable message on failure.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys


def frame(payload: dict) -> bytes:
    body = json.dumps(payload).encode("utf-8")
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii")
    return header + body


def read_message(stream) -> dict:
    headers = {}
    while True:
        line = stream.readline()
        if not line:
            raise RuntimeError("server closed stdout before sending headers")
        line = line.decode("ascii", errors="replace").rstrip("\r\n")
        if line == "":
            break
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()
    length = int(headers["content-length"])
    body = stream.read(length)
    return json.loads(body.decode("utf-8"))


def main() -> int:
    # Honor RUXEN_WORKSPACE (dev builds) and the installed release layout.
    workspace = os.environ.get("RUXEN_WORKSPACE")
    candidates = []
    if workspace:
        candidates.append(os.path.join(workspace, "target", "release", "ruxen-lsp"))
    ruxen_home = os.environ.get("RUXEN_HOME") or os.path.expanduser("~/.ruxen")
    candidates.append(os.path.join(ruxen_home, "bin", "ruxen-lsp"))

    bin_path = next((p for p in candidates if os.path.isfile(p)), None)
    if bin_path is None:
        print(f"ruxen-lsp not found; looked in: {candidates}", file=sys.stderr)
        return 2

    proc = subprocess.Popen(
        [bin_path],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    initialize = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": os.getpid(),
            "rootUri": None,
            "capabilities": {},
            "clientInfo": {"name": "ruxen-e2e", "version": "0.0.1"},
        },
    }
    shutdown = {"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": None}
    exit_n  = {"jsonrpc": "2.0", "method": "exit", "params": None}

    try:
        proc.stdin.write(frame(initialize))
        proc.stdin.flush()
        response = read_message(proc.stdout)
        if "result" not in response:
            print(f"no `result` in initialize response: {response}", file=sys.stderr)
            return 3
        caps = response["result"].get("capabilities")
        if not isinstance(caps, dict):
            print(f"`capabilities` missing or wrong type: {response['result']}",
                  file=sys.stderr)
            return 4
        print(f"ok: ruxen-lsp replied with {len(caps)} capability field(s)")

        # Polite shutdown so the server doesn't hang on a pipe close.
        proc.stdin.write(frame(shutdown))
        proc.stdin.flush()
        try:
            read_message(proc.stdout)
        except Exception:
            pass
        proc.stdin.write(frame(exit_n))
        proc.stdin.flush()
        proc.stdin.close()
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()
        return 0
    except Exception as e:
        print(f"error: {e}", file=sys.stderr)
        proc.kill()
        return 5


if __name__ == "__main__":
    sys.exit(main())
