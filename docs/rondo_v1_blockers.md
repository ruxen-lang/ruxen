# Ruxen plumbing needed for Rondo v1.0 (full Sinatra parity)

Rondo v0.7 (in `~/.projects/rondo`) is Sinatra-feature-complete
for the common-case surface: route DSL, path/query params,
body parsing, before/after hooks, error handlers per status,
cookies (read + set), redirect, HEAD auto-handling, static
files with path-traversal protection, and a live blocking TCP
server.

This doc lists the Ruxen-side work that has to land before
Rondo can ship the remaining Sinatra features. Items are
ordered roughly by how many real-world apps they block.

Every item lists:
- **What Rondo can't do today** without it
- **Where the fix lives** (existing file, new file, stdlib, runtime)
- **A repro / acceptance test** so the fix can be pinned

Cross-reference: rondo's own `docs/ruxen-issues.md` catalogues
the workarounds rondo ships today; this doc is the inverse —
what Ruxen needs to add.

---

## P0 — blocks production-shape Rondo apps

### B1. Per-connection threading actually works for an accept-loop spawn

**What Rondo can't do today:** serve more than one client at a
time. `Rondo.listen` is a sequential `accept → handle → close`
loop. A slow request blocks every other client.

**What's needed:** validate that `Thread.spawn { handle_one(...) }`
on each accepted TcpStream works end-to-end. The Ruxen stdlib
exposes `Thread.spawn` and Codex's recent commits added a
JoinHandle surface — but nobody has confirmed the per-connection
threading shape works with:
1. A closure that captures the `app: &Rondo` reference
2. A moved-in `stream: TcpStream`
3. Independent drop semantics per connection (the thread's
   drop frees the stream; the accept loop keeps the listener)

Memory file `project_ruxen_task_spawn_ownership_gap.md`
documents that `Task.spawn` already has a drop-elaboration
gap; the same shape on `Thread.spawn` is the likely failure
mode.

**Acceptance test:**

```ruxen
def main
  let listener = TcpListener.bind(&"127.0.0.1:8421").unwrap()
  loop
    match listener.accept
      Ok(stream) -> Thread.spawn({ || handle(stream) })
      Err(_)     -> break
    end
  end
end

def handle(s: TcpStream)
  # echo a fixed line and close
  let bytes = "ok\n".bytes()
  let _ = s.write(&bytes)
  let _ = s.close
end
```

Should:
- Build cleanly (closure captures `s` by move).
- Serve `for i in $(seq 1 100); do curl localhost:8421/ & done`
  with no double-frees, no leaks, no crashes.
- Run a 10-second `wrk -t4 -c100` without crashing.

### B2. Three async compiler gaps from the original quality review

Tracked in memory `project_ruxen_async_compiler_gaps.md` and
`docs/quality_review.md` §1.3. Repeated here for the
Rondo-blocking framing:

#### B2a. `block_on` Output type erasure
Ruxen's `block_on(future)` returns the future's `Output` type,
but the wrapper currently erases it back to a fresh inference
variable. So `let n = block_on(some_int_future)` reads `n` as
`?T`, not `Int`. Once fixed, Rondo can drop `Rondo.listen`'s
threading dependency entirely and run on the executor.

#### B2b. `.method.await` doesn't resolve
`stream.read().await` should desugar to `await(stream.read())`
but typeck doesn't see through it — the method call resolves
fine on its own, but in the awaitee position the call's type
is dropped. Rondo's async-net port needs this.

#### B2c. `String ==` falls back to pointer-eq after `Result` destructuring
Already flagged in the quality review (gap #3). Rondo's
header / cookie / path comparisons currently work because they
read from direct fields. The async path is where this would
bite — `match read_result; Ok(line) -> if line == "..." then ...`
silently wrong-comparing.

**Acceptance test:** the existing async fixtures under
`compiler/ruxen_core/tests/fixtures/ruxen/async_*.rx` plus a
new fixture that does `async def handle(s: AsyncTcpStream)`
mirroring Rondo's handle_one shape against
`std.async_net.AsyncTcpListener`.

### B3. JSON parser in stdlib

**What Rondo can't do today:** parse a JSON request body. Every
real API endpoint does `JSON.parse(req.body)` somewhere. We
currently hand back `req.body: String` and the user is on
their own.

**What's needed:** a `std::json` package with at minimum:
- `Json.parse(&String) -> Result[JsonValue, JsonError]`
- `Json.stringify(&JsonValue) -> String`
- `JsonValue` enum with `Null | Bool(Bool) | Num(Float) | Str(String) | Array(Array[JsonValue]) | Object(Map[String, JsonValue])`
- Helper accessors: `value.get("key")`, `value.as_string`, etc.

Pure-Ruxen implementation is fine — no FFI needed. The parser
is a few hundred lines of code.

**Acceptance test:** round-trip the standard JSON test suite
(the small subset that ships with Ruby's `json` gem tests).
Rondo will pin against `Json.parse(req.body)?.get("name")?
.as_string`.

### B4. HMAC-SHA256 (or equivalent) for signed sessions

**What Rondo can't do today:** offer signed session cookies.
Without HMAC, sessions are either (a) stored entirely
server-side keyed by an unguessable session-id cookie, or
(b) trivially forgeable. Option (a) means in-memory map →
no horizontal scaling. Option (b) is a non-starter.

**What's needed:** a `std::crypto` package (or extension of
existing `std::hash`) with at minimum:
- `HmacSha256.sign(&key: String, &msg: String) -> String` (hex-encoded)
- `HmacSha256.verify(&key: String, &msg: String, &mac: String) -> Bool`
  (constant-time comparison)

The C runtime can shell out to a single-file SHA-256 + HMAC
impl (~300 LOC each, plenty of public-domain references). No
external dep needed.

**Acceptance test:** signed cookie round-trip — sign a
`session=abc` payload, verify it, mutate one byte, verify
returns false.

---

## P1 — blocks "complete" Rondo, but apps can ship without

### B5. TLS / HTTPS support

**What Rondo can't do today:** serve HTTPS. Every public-
facing modern app needs this; today users would terminate TLS
in nginx/Caddy in front of Rondo.

**What's needed:** `std::tls` wrapping either OpenSSL or
rustls. The runtime surface should mirror `std::net`:
- `TlsListener.bind(addr, cert, key) -> Result[TlsListener, IoError]`
- `TlsListener.accept() -> Result[TlsStream, IoError]`
- `TlsStream.read` / `TlsStream.write` / `TlsStream.close`

Rondo would then add `Rondo.listen_tls(addr, cert, key)`
parallel to the existing `Rondo.listen`.

This is a substantial dep / FFI surface. Worth pinning the
choice (rustls is the modern call for cargo-style projects;
OpenSSL is the broader-compatibility call) before starting.

**Acceptance test:** the existing rondo-smoke `serve` mode
gets a `serve-tls` variant; `curl --cacert ... https://...`
returns the same responses as the plaintext path.

### B6. Regex engine in stdlib

**What Rondo can't do today:** offer Sinatra's regex-route
shape `get %r{/post/\d+}`. Today only `:name` segments work.

**What's needed:** `std::regex` (or `std::re`) with at minimum:
- `Regex.compile(&String) -> Result[Regex, RegexError]`
- `regex.matches(&String) -> Bool`
- `regex.captures(&String) -> Option[Array[String]]`

POSIX-ish flavour is plenty for v1. Could be a pure-Ruxen
port of a small regex engine (a few hundred LOC) or an FFI
binding to PCRE2.

**Acceptance test:** `Regex.compile(&"/post/(\\d+)").captures(&"/post/42")`
returns `Some(["/post/42", "42"])`.

### B7. Multipart form-data parser

**What Rondo can't do today:** accept file uploads. Today
`req.body` is the raw bytes; without a multipart parser you
can't extract individual form fields or file parts.

**What's needed:** a parser (pure-Ruxen is fine, ~200 LOC)
that takes:
- `req.body: String`
- `Content-Type: multipart/form-data; boundary=...`

…and returns `Vec[MultipartPart]` where each part has a name,
optional filename, headers, and body bytes (or String).

This sits naturally inside Rondo, but the implementation
needs `String.split_bytes_at(&[Int]) -> Vec[String]` (or
similar byte-aware slicing) — String's current `split` is
delimiter-string based and not safe for arbitrary boundaries
that include `\r\n--`. Either expose that, or expose enough
Vec[Int] manipulation that Rondo can roll its own.

**Acceptance test:** parse a `curl -F file=@foo.txt -F name=bar`
request body; extract `name → "bar"` and `file → "<contents of foo.txt>"`.

---

## Items considered and dropped

A first draft also listed these as P2 "ergonomic gaps." I
went through each against two filters — (1) does Rondo today
have a working workaround, and (2) does Sinatra/Ruby actually
have the feature being requested — and dropped every one of
them:

| Item | Why dropped |
|---|---|
| `derive Clone` keyword | Ruby has `Object#dup` / `#clone` as universal methods (not a derive macro). Rondo doesn't need it today — `&Request` hook signatures sidestep clone entirely. If a class ever needs a copy, write `def clone -> Foo; var r = Foo.new; r.field = self.field; r end` by hand, same as Ruby's `def dup`. |
| Forward refs in helper signatures | Workaround in place (helpers live at the end of `lib.rx`). Ruby is dynamic so the question doesn't arise the same way. |
| Multi-stmt match arm needs leading `let` | Workaround in place (`let _captured = params`). Ruby's `case`/`when` is a different shape entirely, so there's no Ruby parity argument. |
| `()` as no-op match arm | Rust-style ask. Ruby has `nil` for the same role; Rondo uses `None -> 0` which works fine. |
| Path-dep modules (`use rondo.X`) | Flat-merge works today. Ruby has `require` + `Rondo::Class`, but Rondo doesn't NEED the namespacing — names don't collide. |
| `String.split` returns SplitIter on `Str` | Rondo never hits this — every split receiver is an owned `String`. Ruby's `String#split` is consistent (always Array), so this is a real Ruxen inconsistency, but it doesn't block Rondo. |
| `Some("ada")` coerce to `Option[String]` | Rust-style ask (Ruby has no Option type). Rondo writes `Some(String.from(&"..."))` and moves on. |

Anything listed above is fair game to fix later if it bites
another project — but they're not Rondo blockers.

---

## Tracking

Memory files in `/Users/hassan/.claude/projects/-Users-hassan--projects-ruxen/memory/`:
- `project_ruxen_async_compiler_gaps.md` — B2a/b/c
- `project_ruxen_task_spawn_ownership_gap.md` — B1
- `feedback_no_redundant_rebind` — adjacent style

Rondo issues file (the inverse view): `~/.projects/rondo/docs/ruxen-issues.md`.

Latest related upstream commits (in this branch):
- `b58b6fe` mir(drops): closure-call returns-self elide
- `23b41c5` typeck(infer): .call on dyn-erased Fn receivers
- `0ad8082` stdlib(option_result): Option/Result unwrap + expect
- `6e12d48` parser(closure): multi-stmt closure bodies
- `a16c09b` stdlib(string): String.from_bytes
- `c928c63` mir(field_access): dotted-class FFI alias
- `80acb14` mir(match): Option Some variant_idx off-by-one
- `0257989` parser(match): nested multi-stmt match arm
- `9c7028b` typeck(infer): pattern-binding type propagation
- `0a12b18` mir(drops): UAF dealloc elide for returns-self
- `d530ad9` core(stdlib): embed bootstrap .rx via include_str!
