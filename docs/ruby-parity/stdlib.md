# Ruby standard-library parity — audit matrix

Authoritative, no-skip backlog for **Ruby's bundled standard library** (the
`require`-able libraries that ship with CRuby/MRI, *not* core classes — those
live in [`core.md`](core.md) — and *not* third-party gems — those live in
`gems-*.md`). Every library gets a table; every real public API gets a row.

Read [`README.md`](README.md) first for the tier framework. Recap of the status
values used in every table below:

| Status | Meaning |
|--------|---------|
| **DONE** | Full parity exists in Ruxen today (module named). |
| **PARTIAL** | Some parity exists; the gap is spelled out in the Note. |
| **MISSING** | Not implemented. The row scopes what a parity module must provide. |
| **COMPILE-TIME** | Ruby does it with load-time/runtime metaprogramming; Ruxen re-expresses it with compile-time synthesis (`derive`/macro/mixin/codegen). The row names the mechanism. |
| **OUT-OF-SCOPE** | Needs a live runtime Ruxen deliberately lacks (`eval`/`binding`/reflection/GC). The row names the tier-2 substitute. |

**What exists today** (`library/std/`): `json`, `set`, `time` (Instant/Duration
+ `Time.unix_ns` only), `rand`, `regex`, `net` (blocking TCP), `async_net`,
`http` (1-method blocking GET client), `fmt`, `io`, `fs`, `path`, `env`,
`process`, `sync` (threads/mutex/atomics/channels), `future` (async runtime),
`bufio`, `bench`. Everything crypto/encoding/URL/date/serialization-format is
absent and forms the bulk of this backlog.

---

## 1. `json` — DONE (with option gaps) · `library/std/json`

Ruxen models JSON as an opaque C-backed `Json` node with a static `JSON` facade
(parse/stringify + `make_*` builders + `is_*`/`as_*` inspectors). Parsing and
generation are solid; the gaps are Ruby's option keywords and automatic
object↔JSON mapping (which is a compile-time serializer job, cross-ref
`gems-db.md` serializers).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `JSON.parse(str)` | DONE | `JSON.parse` → `Result[Json, JsonError]` | Ruby raises; Ruxen returns `Result`. |
| `JSON.parse(str, symbolize_names: true)` | MISSING | — | No `Symbol` distinction; keys stay `String`. Low value in a typed lang. |
| `JSON.parse(str, max_nesting: n)` | PARTIAL | `JsonError::DepthLimit` | Fixed depth limit; not caller-configurable. |
| `JSON.parse(str, allow_nan: true)` | MISSING | — | NaN/Infinity extension not accepted. |
| `JSON.parse(str, create_additions: true)` | OUT-OF-SCOPE | — | Needs runtime class dispatch on `json_class`; use a compile-time deserializer. |
| `JSON.parse!(str)` | PARTIAL | `JSON.parse_strict` | Strict variant exists; semantics differ (not "no limits"). |
| `JSON.generate(obj)` | DONE | `JSON.stringify(&Json)` → `Result[String]` | Operates on a `Json` node, not arbitrary objects. |
| `JSON.generate(obj, opts)` (indent/space/space_before/object_nl/array_nl) | MISSING | — | No fine-grained whitespace options; only pretty vs compact. |
| `JSON.pretty_generate(obj)` | DONE | `JSON.stringify_pretty(&Json, indent: Int)` | Indent width is a param. |
| `JSON.dump(obj[, io[, limit]])` | MISSING | alias of `stringify` | Add `JSON.dump` alias; `io`/`limit` args need `StringIO`/`File` sink. |
| `JSON.load(source[, proc[, opts]])` | MISSING | alias of `parse` | Add `JSON.load`; `proc` transform + `create_additions` out of scope. |
| `JSON.load_file(path)` / `JSON.dump_file` | MISSING | — | Compose `fs.read_to_string` + `parse`; add convenience. |
| `Object#to_json` | COMPILE-TIME | — | Derive `to_json` per struct/enum from fields (serde-style codegen). |
| `String#to_json`, `Integer#to_json`, … (core `to_json`) | COMPILE-TIME | — | Same derive; primitives map to `JSON.make_*`. |
| `JSON[str]` / `JSON[obj]` (round-trip shortcut) | MISSING | — | Sugar over parse/generate. |
| `JSON.generator` / `JSON.parser` / `JSON.create_id` | OUT-OF-SCOPE | — | Pluggable backend selection; not applicable to a single C backend. |
| Node inspection: `.kind`, `.is_*`, `.as_*`, `.array_len/get`, `.object_len/has/get` | DONE | `JSON.*` facade | Richer than Ruby (Ruby returns plain `Hash`/`Array`); Ruxen keeps a typed node. |
| Builders: `null_value/bool/int/float/string/array/object/empty_*` | DONE | `JSON.*` | No Ruby analog; used to construct nodes for generation. |

---

## 2. `set` — PARTIAL · `library/std/set`

`Set[T]` exists with the core membership + boolean-algebra operations. Gaps are
the mutating-with-answer variants (`add?`/`delete?`), the comparison predicates
(subset/superset/disjoint), symmetric difference, `merge`/`subtract`, and the
whole `Enumerable` surface (Ruby's `Set` includes `Enumerable`).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Set.new(enum=nil)` | PARTIAL | `Set.new` (empty) / `Set.with_capacity` | No "from enumerable" ctor; add `Set.from(Array[T])`. |
| `Set[a, b, …]` literal | MISSING | — | Needs a variadic/array constructor sugar. |
| `#add(o)` / `#<<` | PARTIAL | `#insert(item)` | Rename/alias to `add`/`<<` for Ruby ergonomics. |
| `#add?(o)` → self or nil | MISSING | — | Insert returning whether it was new. |
| `#delete(o)` | PARTIAL | `#remove(item) -> Bool` | Ruby `delete` returns self; `delete?` returns nil/self. Map. |
| `#delete?(o)` | PARTIAL | `#remove` returns Bool | Semantics close; align return. |
| `#delete_if` / `#reject!` / `#keep_if` / `#select!` | MISSING | — | Needs block predicate + in-place. |
| `#include?` / `#member?` / `#===` | PARTIAL | `#include?` | Add `member?`/`===` aliases. |
| `#size` / `#length` / `#count` | PARTIAL | `#size` | Add `length` alias; `count` from Enumerable. |
| `#empty?` | DONE | `#empty?` | |
| `#clear` | DONE | `#clear` | |
| `#each` | DONE | free-fn `each` | Present; wire into an Enumerable mixin. |
| `#to_a` | DONE | `#to_a` → `Array[&T]` | |
| `#to_set` | MISSING | — | Trivial identity/convert. |
| `#|` / `#union` / `#+` / `#merge` | PARTIAL | `#union` | Add operator aliases + `merge` (mutating union). |
| `#&` / `#intersection` | PARTIAL | `#intersection` | Add `&` alias. |
| `#-` / `#difference` / `#subtract` | PARTIAL | `#difference` | Add `-` alias + `subtract` (mutating). |
| `#^` (symmetric difference) | MISSING | — | `(a|b) - (a&b)`. |
| `#subset?` / `#<=` | MISSING | — | Boolean predicate. |
| `#proper_subset?` / `#<` | MISSING | — | |
| `#superset?` / `#>=` | MISSING | — | |
| `#proper_superset?` / `#>` | MISSING | — | |
| `#disjoint?` / `#intersect?` | MISSING | — | |
| `#==` / `#hash` | MISSING | — | Value equality across two sets. |
| `#replace(enum)` | MISSING | — | |
| `#classify {block}` → Hash | MISSING | — | Enumerable-style grouping. |
| `#divide {block}` → Set of Sets | MISSING | — | Tarjan partition; low priority. |
| `#flatten` / `#flatten!` | MISSING | — | Set-of-sets flatten. |
| `Enumerable` methods (`map`/`select`/`reduce`/`min`/`max`/`sum`/`group_by`/`sort`/…) | MISSING | — | Include the Enumerable mixin (see §21) over `each`. |
| `SortedSet` | OUT-OF-SCOPE | — | Removed from Ruby stdlib in 3.0 (now the `sorted_set` gem). Track under gems if demanded. |

---

## 3. `date` / `time` — MISSING (large) · `library/std/time` has monotonic clock only

**The biggest single stdlib gap.** Ruxen's `time` module provides only
`Instant`/`Duration` (a Rust-style *monotonic* clock) and `Time.unix_ns` (a raw
wall-clock nanosecond count). There is **no calendar `Date`, no `DateTime`, no
civil `Time`, no `strftime`/`strptime`, no timezone handling, no calendar
arithmetic**. A parity module must build all of it. This is P0 for any real app
(records have `created_at`, APIs emit ISO-8601).

### 3a. `Time` (core class; `strftime`/parse from the `time` stdlib)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Time.now` | MISSING | `Time.unix_ns` (raw ns only) | Need a civil `Time` value carrying y/m/d/h/m/s/ns + offset. |
| `Time.at(seconds[, subsec])` | MISSING | — | Construct from epoch. |
| `Time.new(y,m,d,h,mi,s, tz)` / `Time.local` / `Time.mktime` | MISSING | — | Local-zone construction. |
| `Time.utc(...)` / `Time.gm(...)` | MISSING | — | UTC construction. |
| `Time.parse(str)` (time stdlib) | MISSING | — | Heuristic parse; key for ingest. |
| `Time.strptime(str, fmt)` (time stdlib) | MISSING | — | Format-directed parse. |
| `Time.iso8601(str)` / `Time.xmlschema` | MISSING | — | ISO-8601 ingest. |
| `#year #month/#mon #day/#mday #hour #min #sec #usec #nsec #subsec` | MISSING | — | Field accessors. |
| `#wday #yday #monday?..#sunday?` | MISSING | — | Weekday accessors. |
| `#to_i #to_f #to_r` | MISSING | — | Epoch conversions. |
| `#+ (sec)` / `#- (sec or Time)` | MISSING | — | Arithmetic → `Time` or `Float` seconds. |
| `#<=> #== #eql? #hash` | MISSING | — | Comparable ordering. |
| `#strftime(fmt)` | MISSING | — | The formatting workhorse (`%Y-%m-%d %H:%M:%S %z …`). |
| `#iso8601` / `#xmlschema` / `#rfc2822` / `#httpdate` / `#rfc3339` | MISSING | — | Canonical string emitters. |
| `#utc? #utc/#getutc #localtime/#getlocal #zone #utc_offset/#gmt_offset` | MISSING | — | Zone handling. |
| `#dst?` | MISSING | — | DST flag (needs tz database). |
| `#round(n) #ceil(n) #floor(n)` | MISSING | — | Subsecond rounding. |
| `Time::Comparable` (`between?`, `clamp`) | MISSING | — | Via Comparable mixin (§21). |

### 3b. `Date` (the `date` stdlib)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Date.new(y,m,d)` / `Date.civil` | MISSING | — | Proleptic Gregorian civil date. |
| `Date.today` | MISSING | — | |
| `Date.parse(str)` / `Date.iso8601` / `Date.strptime(str, fmt)` | MISSING | — | Parsing. |
| `Date.jd(n)` / `Date.ordinal` / `Date.commercial` | MISSING | — | Alt calendars/ordinals. |
| `#year #month/#mon #day/#mday #wday #yday #cwday #cweek #cwyear` | MISSING | — | Field accessors. |
| `#leap? #julian? #gregorian?` | MISSING | — | Calendar predicates. |
| `#+ (days) #- (days or Date→Rational) #<< (months) #>> (months)` | MISSING | — | Arithmetic. |
| `#next_day #prev_day #next_month #prev_month #next_year #prev_year #succ/#next` | MISSING | — | Stepping. |
| `#step(limit, step) #upto #downto` | MISSING | — | Iteration. |
| `#<=> #== #eql?` | MISSING | — | Comparable. |
| `#strftime #iso8601 #rfc3339 #httpdate #jd #mjd #ajd` | MISSING | — | Formatting/serials. |
| `#to_time #to_date #to_datetime` | MISSING | — | Conversions. |
| `#monday?..#sunday?` | MISSING | — | Weekday predicates. |

### 3c. `DateTime` (the `date` stdlib; `Date` subclass with time-of-day)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `DateTime.new(y,m,d,h,mi,s,offset)` / `.now` / `.parse` / `.strptime` / `.iso8601` | MISSING | — | Construction/parse with offset. |
| `#hour #min/#minute #sec/#second #sec_fraction #zone #offset` | MISSING | — | Time-of-day + zone. |
| `#new_offset(off)` | MISSING | — | Re-zone. |
| `#to_time #to_date` | MISSING | — | Conversions. |
| `#strftime #iso8601 #rfc3339 #httpdate` | MISSING | — | Formatting. |

**Parity module must minimally provide:** a civil `Time` type (fields + offset),
epoch↔civil conversion (leap-year/Zeller math), `strftime` (the full directive
set), `strptime`/`parse` for ISO-8601 + RFC-2822 + common formats, arithmetic
(`+`/`-`/`between?`), and the canonical emitters (`iso8601`/`rfc2822`/`httpdate`
— the last two are required by `net/http`). Full IANA-tz/DST can be a follow-up
(start with fixed-offset + UTC + local).

---

## 4. `securerandom` — PARTIAL · `library/std/rand`

`Rand` gives CSPRNG bytes (`random_bytes`, `random_u64`, `random_fill`). The
Ruby-facing conveniences (hex, base64, **uuid**, bounded integer) are missing —
and `uuid` is a P0 for app primary keys.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `SecureRandom.random_bytes(n=16)` | DONE | `Rand.random_bytes(n)` → `Result[Array[Int]]` | |
| `SecureRandom.bytes(n)` | DONE | `Rand.random_bytes` | Alias. |
| `SecureRandom.hex(n=16)` | MISSING | — | Bytes → lowercase hex (needs a hex encoder; shared with `digest`). |
| `SecureRandom.base64(n=16)` | MISSING | — | Needs `base64` (§7). |
| `SecureRandom.urlsafe_base64(n=16, padding=false)` | MISSING | — | Needs `base64.urlsafe_*`. |
| `SecureRandom.uuid` / `uuid_v4` | MISSING | — | **P0.** 16 random bytes → v4 UUID string. |
| `SecureRandom.uuid_v7` | MISSING | — | Time-ordered UUID (Ruby 3.3+); needs wall clock (§3). |
| `SecureRandom.random_number(n=0)` | MISSING | — | `n` Integer → `[0,n)`; `n` Float → `[0.0,n)`; `0` → `[0.0,1.0)`. |
| `SecureRandom.alphanumeric(n=16, chars:)` | MISSING | — | Random `[A-Za-z0-9]` string. |
| `SecureRandom.rand` / `SecureRandom.choose` | MISSING | — | `rand` alias of `random_number`; `choose` (3.3+) picks from a charset. |
| `SecureRandom.gen_random(n)` | DONE | `Rand.random_bytes` | Internal primitive; same thing. |
| Non-secure `Random` / `Kernel#rand` / `Kernel#srand` / `Array#sample` / `#shuffle` | MISSING | — | Core PRNG (seedable, reproducible) — cross-ref `core.md`; distinct from CSPRNG. |

---

## 5. `digest` — MISSING · no module

Cryptographic + checksum hashing. Zero coverage today. Needed for
password/token hashing, ETags, cache keys, HMAC signing (webhooks, JWT). A
parity module wraps a C hash backend and exposes both the one-shot class methods
and the streaming instance API.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Digest::MD5.hexdigest(s)` / `.digest(s)` / `.base64digest(s)` | MISSING | — | MD5 (non-crypto uses: ETag/cache key). |
| `Digest::SHA1.hexdigest/.digest/.base64digest` | MISSING | — | |
| `Digest::SHA256.hexdigest/.digest/.base64digest` | MISSING | — | **P0** (tokens, signatures). |
| `Digest::SHA384` / `Digest::SHA512` / `Digest::SHA2.new(bitlen)` | MISSING | — | |
| `Digest::RMD160` / `Digest::BubbleBabble` | MISSING | — | Low value. |
| `Digest::CLASS.file(path)` | MISSING | — | Hash a file (stream from `fs`/`io`). |
| `#new` / `#update(s)` / `#<<` | MISSING | — | Streaming/incremental hashing. |
| `#digest` / `#hexdigest` / `#base64digest` | MISSING | — | Finalize (raw / hex / base64). |
| `#reset` | MISSING | — | Reuse the context. |
| `#digest_length` / `#block_length` | MISSING | — | Algorithm metadata. |
| `#==` (constant-time compare) | MISSING | — | Digest equality. |
| `Digest.hexencode(str)` / `Digest.bubblebabble` | MISSING | — | Byte→hex helper (shared with SecureRandom.hex). |
| `OpenSSL::HMAC.hexdigest(digest, key, data)` / `.digest` | MISSING | — | **P0** — HMAC (webhook signatures, JWT HS256). Lives in `openssl` (§8) in Ruby but pair it here. |

---

## 6. `base64` — DONE · `library/std/base64`

The `base64` package exists (`class Base64`, FFI). Full 6-method surface.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Base64.encode64(bin)` | DONE | `Base64.encode64` | MIME variant (line-wrap semantics unverified). |
| `Base64.decode64(str)` | DONE | `Base64.decode64` | |
| `Base64.strict_encode64(bin)` | DONE | `Base64.strict_encode64` | |
| `Base64.strict_decode64(str)` | DONE | `Base64.strict_decode64` | |
| `Base64.urlsafe_encode64(bin, padding: true)` | DONE | `Base64.urlsafe_encode64` | |
| `Base64.urlsafe_decode64(str)` | DONE | `Base64.urlsafe_decode64` | |

---

## 7. `uri` — MISSING · no module

URL parsing/construction + `www-form` encoding. Needed by `net/http` (§9), form
bodies, query strings, OAuth, routing. `CGI.escape`-style component encoding
overlaps with `cgi` (§10) — share one percent-codec.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `URI.parse(str)` / `URI(str)` | MISSING | — | Returns a typed URI (HTTP/HTTPS/Generic). Ruxen: one `Uri` struct with a `scheme`. |
| `URI.join(base, *rel)` | MISSING | — | Resolve relative refs (RFC-3986). |
| `URI.encode_www_form(enum)` | MISSING | — | `[[k,v],…]` → `a=1&b=2` (P0 for form/query bodies). |
| `URI.decode_www_form(str)` | MISSING | — | Inverse → `[[k,v],…]`. |
| `URI.encode_www_form_component(s)` | MISSING | — | Percent-encode one component. |
| `URI.decode_www_form_component(s)` | MISSING | — | Percent-decode one component. |
| `#scheme #userinfo #user #password #host #hostname #port #path #query #fragment #opaque` | MISSING | — | Component accessors (+ setters in Ruby). |
| `#to_s` | MISSING | — | Reassemble. |
| `#merge(rel)` / `#+` / `#route_to` / `#relative?` / `#absolute?` | MISSING | — | Relative-reference algebra. |
| `#normalize` | MISSING | — | Canonicalize scheme/host case, default port. |
| `#request_uri` (HTTP) | MISSING | — | `path?query` for the request line. |
| `#select(*components)` / `#component` | MISSING | — | Introspection; low value. |
| `URI.extract` / `URI.regexp` | OUT-OF-SCOPE | — | Deprecated in Ruby; skip. |
| `URI::DEFAULT_PARSER` / `RFC2396_Parser` / `RFC3986_Parser` | OUT-OF-SCOPE | — | Pluggable parser objects; a single RFC-3986 parser suffices. |

---

## 8. `openssl` — MISSING · no module (TLS gates HTTPS)

Huge surface. The load-bearing pieces for apps are **TLS** (blocks HTTPS in
`net/http`), **HMAC** (§5), **symmetric ciphers** (session/cookie encryption),
and **digests** (§5). PKI/X.509 is lower priority. All of this binds to a TLS/
crypto backend via FFI (rustls + ring, or system OpenSSL) — a runtime decision,
not a compile-time-synthesis one.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `OpenSSL::SSL::SSLContext.new` (`verify_mode=`, `ca_file=`, `cert=`, `key=`, `min_version=`) | MISSING | — | **P1** — TLS config; required for HTTPS client + server. |
| `OpenSSL::SSL::SSLSocket.new(tcp, ctx)` (`connect`, `accept`, `read`, `write`, `peer_cert`) | MISSING | — | **P1** — wraps `net.TcpStream` in TLS. |
| `OpenSSL::HMAC.hexdigest/.digest(digest, key, data)` | MISSING | — | **P0** (see §5). |
| `OpenSSL::Digest.new(name)` / `OpenSSL::Digest::SHA256` | MISSING | — | Same backend as `digest` (§5). |
| `OpenSSL::Cipher.new("aes-256-gcm")` (`encrypt`/`decrypt`, `key=`, `iv=`, `auth_data=`, `auth_tag`, `update`, `final`, `random_key`, `random_iv`) | MISSING | — | **P1** — encrypted cookies/sessions/secrets. |
| `OpenSSL::PKey::RSA` / `EC` / `DSA` (`generate`, `sign`, `verify`, `public_encrypt`, `private_decrypt`, `to_pem`, `from_pem`) | MISSING | — | Asymmetric crypto (JWT RS256/ES256, key exchange). |
| `OpenSSL::X509::Certificate` / `Name` / `Store` / `Request` / `CRL` | MISSING | — | Cert parsing/validation/issuance. |
| `OpenSSL::PKCS7` / `OpenSSL::PKCS5.pbkdf2_hmac` / `OpenSSL::KDF` (`scrypt`, `hkdf`) | MISSING | — | Password KDFs (scrypt/pbkdf2/hkdf) for auth. |
| `OpenSSL::Random.random_bytes` | DONE (equiv) | `Rand.random_bytes` | Same CSPRNG need; unify. |
| `OpenSSL::BN` (bignum) | OUT-OF-SCOPE | — | Arbitrary-precision integers; cross-ref `bigdecimal`/core Integer. |

---

## 9. `net/http` — PARTIAL (seed only) · `library/std/http` + `rondo` (server)

Ruxen's `http` module is a **single blocking `Http.get(host, port, path)`** with
a hand-rolled response parser (plain HTTP only, no TLS, no headers, no other
verbs). Rondo covers the *server* side. The full Ruby `Net::HTTP` client surface
is essentially unbuilt. This is P1 (right behind date/time + crypto/encoding,
which it depends on: `uri`, `base64` for Basic-Auth, `openssl` for HTTPS).

### 9a. `Net::HTTP` class methods (one-shot)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Net::HTTP.get(uri)` / `.get(host, path, port)` | PARTIAL | `Http.get(host, port, path)` | Exists for plain HTTP; no URI overload, no TLS. |
| `Net::HTTP.get_response(uri)` → response | PARTIAL | returns `Option[HttpResponse]` | Response carries `status`+`body`; no headers map. |
| `Net::HTTP.get_print(uri)` | MISSING | — | Convenience. |
| `Net::HTTP.post(uri, data, header)` | MISSING | — | **P1** POST body + headers. |
| `Net::HTTP.post_form(uri, params)` | MISSING | — | `www-form` POST (needs `uri.encode_www_form`). |
| `Net::HTTP.put/patch/delete(uri, …)` | MISSING | — | Other verbs. |
| `Net::HTTP.start(host, port, opts){|http| }` | MISSING | — | Persistent connection scope. |
| `Net::HTTP.new(host, port)` | MISSING | — | Configurable client object (below). |

### 9b. `Net::HTTP` instance (connection object)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `#start` / `#finish` / `#started?` / `#active?` | MISSING | — | Connection lifecycle (keep-alive). |
| `#get/#post/#put/#patch/#delete/#head/#options(path, …)` | MISSING | — | Verb methods on the connection. |
| `#request(req[, body])` / `#request_get` / `#request_post` | MISSING | — | Send a prebuilt request object. |
| `#use_ssl=` / `#verify_mode=` / `#ca_file=` / `#cert=` / `#key=` / `#ssl_version=` | MISSING | — | **P1** HTTPS (needs `openssl` §8). |
| `#read_timeout=` / `#open_timeout=` / `#write_timeout=` / `#keep_alive_timeout=` | PARTIAL | `net.TcpStream.set_*_timeout` exists | Plumb through to the client. |
| `#set_debug_output(io)` | MISSING | — | Wire logging. |
| `#proxy_*` / `Net::HTTP.Proxy` | MISSING | — | Proxy support; lower priority. |
| `#address` / `#port` / `#ipaddr` | MISSING | — | Introspection. |

### 9c. Request classes + response

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Net::HTTP::Get/Post/Put/Patch/Delete/Head/Options.new(path, headers)` | MISSING | — | Request builders. |
| `req.body=` / `req.body_stream=` / `req.set_form_data(hash)` / `req.set_form(...)` (multipart) | MISSING | — | Request body + form encoding. |
| `req["Header"]=` / `req.add_field` / `req.basic_auth(u,p)` / `req.content_type=` | MISSING | — | Header setters (Basic-Auth needs `base64` §6). |
| `res.code` / `res.message` / `res.body` / `res.read_body` | PARTIAL | `HttpResponse.status`/`.body`/`.ok?` | Have status+body; no streaming read. |
| `res["Header"]` / `res.each_header` / `res.get_fields` / `res.content_type` | MISSING | — | **P1** response header access (parse into a map). |
| `res.value` (raise on non-2xx) | PARTIAL | `HttpResponse.ok?` | Have the predicate; add raising variant. |
| `res.kind_of?(Net::HTTPSuccess/Redirection/ClientError/ServerError)` | MISSING | — | Status-class taxonomy. |
| Automatic redirect following | MISSING | — | App must loop today; add opt-in follow. |
| Chunked transfer-encoding / gzip decoding | MISSING | — | Current parser assumes `Content-Length` + `Connection: close`. |

---

## 10. `cgi` (escaping helpers) — MISSING · no module

Only the escaping utilities are in scope (the full `CGI` request object is
rondo's job). Share the percent-codec with `uri` (§7).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `CGI.escape(str)` | MISSING | — | `application/x-www-form-urlencoded` (space→`+`). |
| `CGI.unescape(str)` | MISSING | — | Inverse. |
| `CGI.escapeHTML(str)` / `CGI.escape_html` / `h` | MISSING | — | **P1** — `&<>"'` → entities (XSS-safe view output; needed by `erb` §13). |
| `CGI.unescapeHTML(str)` / `CGI.unescape_html` | MISSING | — | Inverse. |
| `CGI.escapeURIComponent` / `CGI.unescapeURIComponent` | MISSING | — | RFC-3986 component (space→`%20`). |
| `CGI.escapeElement` / `CGI.unescapeElement` | MISSING | — | Selective element escaping; low value. |
| `CGI.parse(query_string)` → `Hash` | MISSING | — | Overlaps `URI.decode_www_form`. |
| `CGI.new` (params/cookies/headers request object) | OUT-OF-SCOPE | rondo | Server request handling lives in the web framework. |

---

## 11. `csv` — MISSING · no module

Tabular import/export — the standard "load a spreadsheet / export a report"
path. P1 for data apps. Build on `stringio` (§17) for the in-memory case.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `CSV.parse(str, **opts)` → `Array[Array]` (or rows) | MISSING | — | Full-string parse. |
| `CSV.parse_line(str, **opts)` → one row | MISSING | — | Single row. |
| `CSV.foreach(path, **opts){|row| }` | MISSING | — | Streaming file read (via `io`). |
| `CSV.read(path, **opts)` | MISSING | — | Read whole file. |
| `CSV.open(path, mode, **opts){|csv| }` | MISSING | — | File handle wrapper. |
| `CSV.generate(**opts){|csv| csv << row }` → String | MISSING | — | Build CSV text. |
| `CSV.generate_line(row, **opts)` | MISSING | — | One row → String (quoting rules). |
| `CSV.new(io_or_string, **opts)` | MISSING | — | Reader/writer object. |
| `#shift` / `#read` / `#each` / `#<<` / `#to_a` | MISSING | — | Row iteration + append. |
| Options: `headers:`, `col_sep:`, `row_sep:`, `quote_char:`, `skip_blanks:`, `return_headers:`, `write_headers:`, `liberal_parsing:` | MISSING | — | At minimum `headers`, `col_sep`, `quote_char`. |
| Options: `converters:` / `header_converters:` | MISSING | — | Typed field conversion; needs a converter registry. |
| `CSV::Row` (`#[]`, `#[]=`, `#field`, `#headers`, `#to_h`, `#to_a`) | MISSING | — | Header-addressed row (P1 ergonomics). |
| `CSV::Table` (`#[]`, `#by_col`, `#by_row`, `#to_a`) | MISSING | — | Table view; lower priority. |

---

## 12. `yaml` (Psych) — MISSING · no module

Config-file format. `safe_load` of scalars/maps/sequences + `dump` covers the
90% (config, fixtures). Full YAML (anchors/aliases/custom tags/merge keys) is a
large spec — scope a subset first.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `YAML.safe_load(str, permitted_classes:, aliases:)` | MISSING | — | **P1** — default `load` since Ruby 3.1. Scalars/Hash/Array/nil/bool/int/float. |
| `YAML.load(str)` | MISSING | — | Alias of `safe_load` in modern Ruby. |
| `YAML.load_file(path)` | MISSING | — | Config read (via `fs`). |
| `YAML.unsafe_load(str)` | OUT-OF-SCOPE | — | Instantiates arbitrary classes at runtime; use a compile-time typed loader instead. |
| `YAML.dump(obj[, io])` | MISSING | — | Emit YAML. Arbitrary-object dump → COMPILE-TIME serializer (like `to_json`). |
| `YAML.load_stream` / `YAML.dump_stream` | MISSING | — | Multi-document. |
| `YAML.parse(str)` / `YAML.parse_stream` → AST | MISSING | — | Node tree; lower priority. |
| Anchors/aliases (`&a` / `*a`), merge keys (`<<`), custom tags (`!ruby/object`) | PARTIAL/OUT-OF-SCOPE | — | Anchors/merge = PARTIAL (parseable); `!ruby/object` tags = out of scope (no runtime class table). |

---

## 13. `erb` — COMPILE-TIME · no module

Templating. Ruby compiles a template string to Ruby code, then `eval`s it
against a `Binding`. Ruxen's design (README tier 2): **compile the template at
build time into a Ruxen function** that takes a typed context struct and returns
a `String`; `<%= expr %>`/`<% code %>` become emitted expressions/statements. No
runtime `binding`. Cross-ref `gems-web.md` (view layer).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `ERB.new(template, trim_mode:, eoutvar:)` | COMPILE-TIME | — | Template → generated render fn. `trim_mode` (`-`/`<>`/`%`) = codegen flag. |
| `#result(binding)` | COMPILE-TIME | — | Replaced by `render(ctx)` over a typed context; no `Binding`. |
| `#result_with_hash(hash)` | COMPILE-TIME | — | Context passed as a struct/hash of typed locals. |
| `#def_method` / `#def_module` / `#def_class` | COMPILE-TIME | — | Template already compiles to a named fn — this is the default shape. |
| `ERB::Util.html_escape` / `h(s)` | MISSING | — | Delegates to `CGI.escapeHTML` (§10). |
| `ERB::Util.url_encode` / `u(s)` | MISSING | — | Delegates to the percent-codec (§7/§10). |
| Runtime template from an unknown string at runtime | OUT-OF-SCOPE | — | Needs `eval`; only build-time-known templates compile. Document the substitute (embed the template file). |

---

## 14. `struct` / `ostruct` — COMPILE-TIME · no module

The canonical compile-time win. `Struct.new(...)` generating a class is exactly
tier 2: emit a Ruxen struct + accessors. `OpenStruct`'s runtime-added fields are
the awkward case (no runtime field insertion) — substitute a compile-time
`Struct`/`Data` or a `Hash`.

### 14a. `Struct` (core-ish, but the `.new`-generates-a-class metaprogramming lands here)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Struct.new(:a, :b) { … }` → class | COMPILE-TIME | `derive`/macro | Generate a nominal struct with the named fields + a block of methods. |
| `Struct.new(:a, keyword_init: true)` | COMPILE-TIME | — | Keyword ctor variant. |
| accessors `#a` / `#a=` | COMPILE-TIME | — | Generated getter/setter per member. |
| `#[](name_or_idx)` / `#[]=` | COMPILE-TIME | — | By-name/index access (index needs a generated match). |
| `#members` / `.members` | COMPILE-TIME | — | Static list of field names. |
| `#to_a` / `#values` / `#to_h` / `#values_at` / `#deconstruct` / `#deconstruct_keys` | COMPILE-TIME | — | Generated conversions (pattern-match support). |
| `#each` / `#each_pair` | COMPILE-TIME | — | Generated iteration over fields. |
| `#== #eql? #hash #dig #size/#length` | COMPILE-TIME | — | Generated structural equality/dig. |
| `Data.define(:a, :b)` (Ruby 3.2+ immutable) → `#with`, `#to_h`, `#deconstruct` | COMPILE-TIME | — | Immutable variant; maps cleanly to a value struct + `with` builder. |

### 14b. `ostruct` (`OpenStruct`)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `OpenStruct.new(hash)` with fields fixed at construction | COMPILE-TIME | — | If the field set is a literal known at compile time, generate a struct. |
| `os.foo` / `os.foo = x` (fields added *after* construction) | OUT-OF-SCOPE | — | Runtime field insertion needs an open object; substitute a `Hash[String, Value]` or a compile-time struct. |
| `#[]` / `#[]=` / `#to_h` / `#each_pair` / `#dig` / `#respond_to?` / `#delete_field` | PARTIAL | — | Provide over the `Hash` substitute; `respond_to?` is compile-time-known. |
| `#== ` / marshal | COMPILE-TIME | — | Structural over the backing map. |

---

## 15. `forwardable` — COMPILE-TIME · no module

Method delegation. Ruby installs forwarding methods at load time via
`def_delegator`. Ruxen generates them at compile time (the accessor + target
method name are literals).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `extend Forwardable` | COMPILE-TIME | — | Marker for the codegen; no runtime module. |
| `def_delegator(accessor, method, ali = method)` | COMPILE-TIME | — | Generate `def ali(*a) = accessor.method(*a)`. |
| `def_delegators(accessor, *methods)` | COMPILE-TIME | — | Batch of the above. |
| `delegate(method => accessor, …)` / `delegate([m1,m2] => accessor)` | COMPILE-TIME | — | Hash form. |
| `instance_delegate` | COMPILE-TIME | — | Same generation. |
| `SingleForwardable` / `single_delegate` | COMPILE-TIME | — | Delegates on the singleton/module; same mechanism. |

---

## 16. `singleton` — COMPILE-TIME · no module

`include Singleton` → a lazily-created, memoized `.instance` with a private
`new`. Ruxen generates a module-level accessor (with a `sync`-guarded init for
thread safety).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `include Singleton` | COMPILE-TIME | — | Generate the instance holder + accessor; make `new` private. |
| `.instance` | COMPILE-TIME | — | Memoized singleton getter (thread-safe init via `sync`/atomic). |
| private `.new`, disabled `#dup`/`#clone` | COMPILE-TIME | — | Compile-time visibility + deleted copy path. |
| `._load` / `#_dump` (marshal hooks) | OUT-OF-SCOPE | — | Marshal integration; skip. |

---

## 17. `stringio` — MISSING · no module

In-memory IO with the `IO` interface. A dependency of `csv`/`erb`/`json` output
and of any code that wants a `File`-shaped sink without touching disk. P2 but
cheap and widely useful.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `StringIO.new(str = "", mode = "r+")` | MISSING | — | Backing buffer + cursor. |
| `#read([len])` / `#gets` / `#readline` / `#readlines` / `#each_line` | MISSING | — | Reading. |
| `#getc` / `#ungetc` / `#getbyte` | MISSING | — | Char/byte reads. |
| `#write(s)` / `#<<` / `#puts` / `#print` / `#printf` | MISSING | — | Writing. |
| `#string` | MISSING | — | Get the accumulated buffer. |
| `#rewind` / `#seek(off, whence)` / `#pos` / `#pos=` | MISSING | — | Cursor control. |
| `#eof?` / `#truncate(n)` / `#size` / `#close` / `#closed?` | MISSING | — | State. |

---

## 18. `pathname` — PARTIAL · `library/std/path` (static string ops only)

Ruxen's `path` module is pure *string* manipulation (`join`, `parent`,
`file_name`, `extname`, `is_absolute`). Ruby's `Pathname` is an object that also
does filesystem IO (delegating to `File`/`Dir`/`FileUtils`). The string half is
partly done; the object + IO half is missing — but the IO primitives already
exist in `fs`/`io`, so this is mostly a wrapper.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Pathname.new(str)` | MISSING | — | Wrap a path string as an object. |
| `#+` / `#join(*p)` | PARTIAL | `Path.join(a,b)` | Have 2-arg join; add variadic + object form. |
| `#parent` / `#dirname` | PARTIAL | `Path.parent` | |
| `#basename([suffix])` / `#file_name` | PARTIAL | `Path.file_name` | No suffix-strip arg. |
| `#extname` | DONE | `Path.extname` | |
| `#sub_ext(ext)` | MISSING | — | Replace extension. |
| `#split` | MISSING | — | `[dirname, basename]`. |
| `#absolute?` / `#relative?` | PARTIAL | `Path.is_absolute` | Add `relative?`. |
| `#cleanpath` / `#expand_path` / `#realpath` / `#realdirpath` | PARTIAL | `fs.canonicalize` | `realpath` ≈ canonicalize; `cleanpath`/`expand_path` (no-IO) missing. |
| `#relative_path_from(base)` | MISSING | — | Compute a relative route. |
| `#each_filename` / `#ascend` / `#descend` | MISSING | — | Component iteration. |
| `#root?` | MISSING | — | |
| `#exist?` / `#directory?` / `#file?` / `#symlink?` | PARTIAL | `fs.exists`/`is_file`/`is_dir` | Wrap onto the object. |
| `#readable?` / `#writable?` / `#executable?` | MISSING | — | Permission probes. |
| `#size` / `#mtime` / `#atime` / `#ctime` / `#birthtime` | PARTIAL | `fs.metadata` | Surface via metadata (times need §3 `Time`). |
| `#read` / `#write` / `#open` / `#each_line` / `#readlines` / `#binread` / `#binwrite` | PARTIAL | `fs.read_to_string`/`write`, `io.File` | Wrap. |
| `#children` / `#entries` / `#glob` / `#opendir` | PARTIAL | `fs.read_dir` | Have flat listing; `glob` (pattern match) missing. |
| `#mkdir` / `#mkpath` / `#rmdir` / `#rmtree` / `#unlink`/`#delete` | PARTIAL | `fs.create_dir`/`create_dir_all`/`remove_dir_all`/`remove_file` | Primitives exist; wrap onto the object. |
| `#rename` / `#chmod` / `#chown` / `#utime` | PARTIAL | `fs.rename` | `chmod`/`chown`/`utime` missing. |
| `#to_s` / `#to_path` | MISSING | — | Unwrap. |

---

## 19. `fileutils` — PARTIAL · `library/std/fs` (primitives present, no FileUtils namespace)

The `fs` module already provides most primitives (`create_dir_all` = `mkdir_p`,
`remove_dir_all` = `rm_rf`, `copy` = single-file `cp`, `rename` = `mv`,
`symlink`, `canonicalize`). Missing: recursive copy (`cp_r`), `touch`, `mv`
across filesystems, `chmod`/`chown`, and the `FileUtils.*` naming.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `FileUtils.mkdir(dir)` | PARTIAL | `fs.create_dir` | |
| `FileUtils.mkdir_p(dir)` / `makedirs` | DONE (equiv) | `fs.create_dir_all` | |
| `FileUtils.rmdir(dir)` | PARTIAL | — | Single empty-dir remove (fs has recursive only). |
| `FileUtils.rm(f)` / `rm_f` | PARTIAL | `fs.remove_file` | `rm_f` (ignore-missing) not distinct. |
| `FileUtils.rm_r(dir)` / `rm_rf` | DONE (equiv) | `fs.remove_dir_all` | |
| `FileUtils.cp(src, dst)` | DONE (equiv) | `fs.copy` | Single file. |
| `FileUtils.cp_r(src, dst)` / `cp_lr` | MISSING | — | Recursive directory copy. |
| `FileUtils.mv(src, dst)` | PARTIAL | `fs.rename` | `rename` fails across filesystems; `mv` should fall back to copy+delete. |
| `FileUtils.ln(old, new)` (hard link) | MISSING | — | |
| `FileUtils.ln_s(old, new)` / `ln_sf` | PARTIAL | `fs.symlink` | Have symlink; `ln_sf` (force) missing. |
| `FileUtils.touch(f)` | MISSING | — | Create/update mtime. |
| `FileUtils.chmod(mode, f)` / `chmod_R` | MISSING | — | Permission bits. |
| `FileUtils.chown(u, g, f)` / `chown_R` | MISSING | — | Ownership. |
| `FileUtils.install(src, dst, mode:)` | MISSING | — | Copy + set mode. |
| `FileUtils.compare_file(a, b)` | MISSING | — | Byte-compare. |
| `FileUtils.copy_stream(src, dst)` | MISSING | — | Stream copy (via `io`). |
| `FileUtils.pwd` / `cd(dir){}` | PARTIAL | `env.current_dir` | Have pwd; `cd` (chdir) missing. |
| `FileUtils.uptodate?(new, old_list)` | MISSING | — | mtime comparison (needs §3). |
| `Dir.glob(pattern)` / `Dir[pattern]` | MISSING | — | Pattern-based listing (also §18 `#glob`); commonly needed. |
| `Dir.mktmpdir` | MISSING | — | Ties to `tempfile` (§20). |
| `Find.find(*paths){|p| }` (the `find` stdlib) | MISSING | — | Recursive walk (fs has flat `read_dir` only). |

---

## 20. `tempfile` — MISSING · no module

Temp files/dirs. Needed for large uploads, atomic writes, test fixtures. Builds
on `fs`/`io` + `securerandom` (unique names) + `env` (tmpdir).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Tempfile.new(basename[, tmpdir])` | MISSING | — | Create + open a uniquely named temp file. |
| `Tempfile.create(basename){|f| }` | MISSING | — | Block form; auto-unlink on exit. |
| `#path` | MISSING | — | The temp path. |
| `#close(unlink_now = false)` / `#close!` / `#unlink` / `#delete` | MISSING | — | Cleanup. |
| `#open` (reopen) / `#length` / `#rewind` | MISSING | — | Reuse. |
| `Dir.mktmpdir([prefix]){|dir| }` | MISSING | — | Temp directory (see §19). |

---

## 21. `comparable` / `enumerable` — COMPILE-TIME (mixin protocols) · cross-ref `core.md`

Both are *mixins* realized in Ruby by including a module that provides default
methods over a single required method (`<=>` for Comparable, `each` for
Enumerable). In Ruxen these are trait-like mixins with default method bodies —
compile-time, not runtime. Full method lists live in [`core.md`](core.md); this
row-set records the protocol and the key surface a parity mixin must provide.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Comparable` via `<=>`: `< <= == > >= between?(a,b) clamp(a,b)` / `clamp(range)` | COMPILE-TIME | core.md | Mixin over a required `<=>`. Set/Time/Date all need it. |
| `Enumerable` via `each`: `map/collect select/filter reject find/detect` | COMPILE-TIME | core.md | Mixin over a required `each` (+ optional `<=>` for min/max/sort). |
| … `reduce/inject each_with_index each_with_object` | COMPILE-TIME | core.md | |
| … `group_by partition chunk chunk_while slice_when tally` | COMPILE-TIME | core.md | |
| … `sort sort_by min max min_by max_by minmax` | COMPILE-TIME | core.md | Needs `<=>`. |
| … `sum count first take drop take_while drop_while` | COMPILE-TIME | core.md | |
| … `flat_map zip each_slice each_cons cycle` | COMPILE-TIME | core.md | |
| … `all? any? none? one? include? find_index` | COMPILE-TIME | core.md | |
| … `to_a/entries to_h to_set uniq` | COMPILE-TIME | core.md | |
| … `lazy` (Enumerator::Lazy) | PARTIAL | `library/std/iter` | Ruxen has an iterator layer; map `lazy` onto it. |

---

## 22. `delegate` (`SimpleDelegator` / `DelegateClass`) — COMPILE-TIME · no module

Object-level delegation. `DelegateClass(Klass)` forwards `Klass`'s public
instance methods (a compile-time-known set → generate forwarders).
`SimpleDelegator` forwards *any* method via `method_missing` — the tier-2
substitute is "forward the statically known method set of the wrapped type."

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `DelegateClass(Klass)` → subclass with forwarders | COMPILE-TIME | — | Generate a forwarder for each public method of `Klass`. |
| `SimpleDelegator.new(obj)` (forward everything) | COMPILE-TIME | — | Forward the wrapped type's known methods; no runtime `method_missing`. |
| `#__getobj__` / `#__setobj__` | PARTIAL/COMPILE-TIME | — | `__getobj__` trivial; `__setobj__` requires same-typed replacement. |
| `Delegator` base / `#method_missing` catch-all | OUT-OF-SCOPE | — | Arbitrary runtime dispatch; substitute is the generated forwarder set. |

---

## 23. `observer` — MISSING · no module (buildable in pure Ruxen)

The Observable mixin is plain runtime state (a list of callbacks) — no
metaprogramming needed. Buildable directly as a mixin holding a `Vec` of
observer closures.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `include Observable` | MISSING | — | Mixin holding the observer list. |
| `#add_observer(obj, func = :update)` | MISSING | — | Register (closure/callback in Ruxen, not a symbol). |
| `#delete_observer(obj)` / `#delete_observers` | MISSING | — | Unregister. |
| `#count_observers` | MISSING | — | |
| `#changed(state = true)` / `#changed?` | MISSING | — | Dirty flag. |
| `#notify_observers(*args)` | MISSING | — | Fire callbacks when changed. |

---

## 24. `logger` — MISSING · no module

Structured logging. Straightforward over `io`/`stdout` + `sync` (thread-safe
writes) + `time` (§3 for timestamps). P1 — every service needs it.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Logger.new(logdev, shift_age = 0, shift_size = ...)` | MISSING | — | Sink = stdout/stderr/file; rotation optional. |
| Levels `DEBUG < INFO < WARN < ERROR < FATAL < UNKNOWN` | MISSING | — | Level enum. |
| `#debug/#info/#warn/#error/#fatal/#unknown(progname){ msg }` | MISSING | — | Level methods (message or block). |
| `#add(sev, msg, progname)` / `#log` | MISSING | — | Generic entry. |
| `#level` / `#level=` / `#debug?`/`#info?`/… | MISSING | — | Threshold. |
| `#formatter=` / `Logger::Formatter` | MISSING | — | Custom line format (closure over sev/time/progname/msg). |
| `#datetime_format=` | MISSING | — | Timestamp format (needs `strftime` §3). |
| `#progname=` / `#<<(raw)` | MISSING | — | Metadata / raw write. |
| `#close` / `#reopen` / `#with_level(lvl){}` | MISSING | — | Lifecycle. |
| `Logger::LogDevice` (rotation by age/size) | PARTIAL | — | Rotation is the harder part; ship non-rotating first. |

---

## 25. `optparse` — MISSING · no module (buildable in pure Ruxen)

CLI option parsing — plain string processing, not metaprogramming, so
directly buildable. P2 (Ruxen apps are mostly servers, but tools need it).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `OptionParser.new { |o| … }` | MISSING | — | Builder. |
| `#on(*opts, desc) { |val| }` / `#on_head` / `#on_tail` | MISSING | — | Declare a flag + handler (closure). |
| `#banner=` / `#separator(str)` / `#program_name=` / `#version=` | MISSING | — | Help/usage text. |
| `#parse!(argv)` / `#parse(argv)` | MISSING | — | Consume args (mutating vs not). |
| `#order` / `#order!` / `#permute` / `#permute!` | MISSING | — | Ordering modes. |
| Typed coercion (`Integer`, `Float`, `Array`, custom via `#accept`) | MISSING | — | Convert option values. |
| Required/optional argument syntax (`--flag VAL` / `[VAL]`) | MISSING | — | Arg presence. |
| `#help` / `#to_s` / `#summarize` | MISSING | — | Generated help. |
| `#getopts(short, *long)` | MISSING | — | One-shot convenience. |
| `--version` / `--help` auto handling | MISSING | — | |
| `OptionParser::ParseError` / `InvalidOption` / `MissingArgument` | MISSING | — | Error types (Ruxen: `Result`). |
| Lighter alternatives `GetoptLong`, `Getopt` | OUT-OF-SCOPE | — | Legacy; OptionParser subsumes. |

---

## 26. `benchmark` — PARTIAL · `library/std/bench`

Ruxen has a `Bencher` (iteration harness + `black_box`), and `time.Instant`
gives wall-clock realtime. Missing: the Ruby `Benchmark.bm`/`bmbm` report format
and the `Tms` (utime/stime/cutime/cstime/real) CPU-time breakdown.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Benchmark.realtime { }` → Float seconds | PARTIAL | `Instant.now`/`.elapsed` | Compose from the monotonic clock. |
| `Benchmark.measure(label) { }` → `Tms` | PARTIAL | `bench.Bencher` | Have an iteration harness; not the `Tms` shape. |
| `Benchmark.bm(width) { |x| x.report(label){} }` | MISSING | — | Tabular report. |
| `Benchmark.bmbm(width) { }` | MISSING | — | Rehearsal + measure. |
| `Benchmark::Tms` (`utime stime cutime cstime real total`, `#+ #- #/ #* #format`) | MISSING | — | Needs CPU-time (getrusage); wall-clock only today. |
| `Bencher#iter` / `black_box` | DONE | `bench.Bencher` | Ruxen-native (Criterion-style), no Ruby analog. |

---

## 27. `thread` / `mutex` / `queue` — PARTIAL · `library/std/sync`

`sync` provides real OS threads (`Thread.spawn`/`JoinHandle`), `Mutex[T]`,
atomics, and channels (`Sender`/`Receiver` ≈ `Queue`), plus `Signal` (condvar-
ish) and `SharedSync`. Gaps vs Ruby: ergonomic `Thread.new`/`#value`/`#name`,
`Thread.list`/`current`/`pass`, a first-class `Queue`/`SizedQueue`, and
`ConditionVariable` with the standard `wait/signal/broadcast`.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Thread.new { }` / `Thread.start` | PARTIAL | `Thread.spawn(closure: Int)` | Exists but takes a raw fn-ptr Int, not an ergonomic typed closure. |
| `#join([limit])` / `#value` | PARTIAL | `JoinHandle.join` | Have join; typed `#value` return + timeout gap. |
| `#alive?` / `#status` / `#stop?` | MISSING | — | Thread state. |
| `Thread.current` / `Thread.main` / `Thread.list` | MISSING | — | Registry (partial: `ThreadId` exists). |
| `Thread.pass` / `Thread.stop` | MISSING | — | Scheduler hints. |
| `#name=` / `#name` | MISSING | — | (`ThreadId` present, not names.) |
| `#kill` / `#raise` / `#wakeup` / `#run` | OUT-OF-SCOPE | — | Async interruption of another thread; no safe equivalent (informs `timeout` §31). |
| `#[]` / `#[]=` (thread-local) / `Thread.current.thread_variable_*` | MISSING | — | Thread-local storage. |
| `#abort_on_exception=` / `Thread.report_on_exception=` | MISSING | — | Panic policy (Ruxen: `ThreadPanic`). |
| `Thread.sleep` / `sleep(sec)` | DONE | `Thread.sleep(&Duration)` | |
| `Mutex#lock/#unlock/#try_lock/#synchronize/#locked?/#owned?` | PARTIAL | `Mutex[T].lock/lock!`, `MutexGuard` | RAII guard model; `synchronize`/`try_lock`/`locked?` gaps. Ruxen's mutex is typed-data-owning (Rust-style), Ruby's guards a critical section. |
| `Mutex#sleep(timeout)` | MISSING | — | Release-and-wait. |
| `Queue` (`#push/#<<`, `#pop/#shift/#deq`, `#size/#length`, `#empty?`, `#clear`, `#close`, `#closed?`, `#num_waiting`) | PARTIAL | `Sender`/`Receiver` (channel) | Channel ≈ unbounded Queue; no unified `Queue` object / `close`/`num_waiting`. |
| `SizedQueue` (`#max`, bounded `#push`) | MISSING | — | Bounded/backpressure channel. |
| `ConditionVariable` (`#wait(mutex, timeout)`, `#signal`, `#broadcast`) | PARTIAL | `Signal` | `Signal` is condvar-adjacent; map to the standard 3-method API. |
| `Thread::Backtrace` / `#backtrace` | OUT-OF-SCOPE | — | Runtime stack introspection. |

---

## 28. `fiber` — PARTIAL (via async) · `library/std/future`

Ruby's `Fiber` (symmetric/asymmetric coroutines: `resume`/`Fiber.yield`/
`transfer`) is the primitive under async. Ruxen's tier-2 substitute is the
compiled **async runtime** (`future`): tasks, `block_on`, `spawn`, `scope`,
`Task.yield_now`, streams, a scheduler. Cooperative concurrency is covered; the
explicit coroutine-handle API (arbitrary `resume`/`yield` with values, symmetric
`transfer`) does not map onto compiled async and is the residual gap.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| Cooperative concurrency / scheduling | DONE (substitute) | `future` (async/await, tasks, scheduler) | The intended Ruxen model. |
| `Fiber.yield(*args)` (cooperative yield point) | PARTIAL | `Task.yield_now` | Yield exists; value passing on resume does not. |
| `Fiber.new { }` / `#resume(*args)` | PARTIAL | `spawn`/`Task` | Task spawn ≈ fiber; no manual step-by-step `resume` with values. |
| `#alive?` / `Fiber.current` | PARTIAL | task handles | Task handle exists; no `Fiber.current` global. |
| `Fiber#transfer` (symmetric coroutines) | OUT-OF-SCOPE | — | Symmetric control transfer; no compiled equivalent. Use channels/tasks. |
| `Fiber.scheduler=` / `Fiber.set_scheduler` (non-blocking IO hook) | PARTIAL | `future` reactor + `async_net`/`async_io` | Ruxen's reactor is the scheduler; it isn't a pluggable Ruby-level object. |
| `Fiber#storage` / `Fiber[]` (fiber-local) | MISSING | — | Task-local storage. |
| `Enumerator` built on Fiber (external iteration) | PARTIAL | `library/std/iter` | Ruxen iterators cover most external-iteration uses. |

---

## 29. `monitor` — MISSING · no module

A **reentrant** mutex + condition variables. Ruxen's `Mutex` is non-reentrant, so
this needs a distinct reentrant lock (owner + recount) plus condvars.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Monitor.new` | MISSING | — | Reentrant lock. |
| `#synchronize { }` / `#mon_synchronize` | MISSING | — | Reentrant critical section. |
| `#mon_enter` / `#mon_exit` / `#mon_try_enter` / `#mon_locked?` / `#mon_owned?` | MISSING | — | Manual lock ops. |
| `#new_cond` → `MonitorMixin::ConditionVariable` (`#wait`, `#wait_while`, `#wait_until`, `#signal`, `#broadcast`) | MISSING | — | Monitor-scoped condvar. |
| `include MonitorMixin` | MISSING | — | Mix reentrant locking into a class. |

---

## 30. `timeout` — MISSING · no module

`Timeout.timeout(sec){ block }` that raises after `sec`. Ruby implements it with
a watcher thread that calls `Thread#raise` on the target — Ruxen has no async
thread interruption (§27), so arbitrary *blocking* code can't be force-cancelled.
Feasible cases: cooperative/async code (cancellation tokens) and syscalls that
take timeouts (socket read/write already do, §9/net).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Timeout.timeout(sec, klass = nil, msg = nil) { }` (blocking code) | PARTIAL/OUT-OF-SCOPE | — | No `Thread#raise`; can't interrupt arbitrary blocking work. |
| Timeout for async tasks | PARTIAL | `future` (`race` / `select` with a sleep) | Race a `Task.sleep` against the work; cooperative cancel. |
| Timeout for socket IO | DONE (equiv) | `net.TcpStream.set_read/write_timeout` | Per-syscall timeouts already exist. |
| `Timeout::Error` / `Timeout::ExitException` | MISSING | — | Ruxen: a `Result`/error variant on the async path. |

---

## 31. `prime` / Integer number theory — MISSING · no module

The `prime` stdlib plus the number-theory Integer methods. (Core `Integer` methods
like `gcd`/`lcm`/`pow(b,m)`/`Integer.sqrt`/`digits`/`bit_length` are tracked in
[`core.md`](core.md); listed here for the app-facing "number theory" cluster.)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Prime.prime?(n)` | MISSING | — | Primality test. |
| `Prime.each(ubound){ |p| }` / `Prime.take(n)` / `Prime.first(n)` | MISSING | — | Prime generation (sieve/lazy). |
| `Prime.prime_division(n)` / `Prime.int_from_prime_division(pd)` | MISSING | — | Factorization ↔ reconstruction. |
| `Integer#prime?` / `Integer.each_prime(ub)` | MISSING | — | Integer-side sugar. |
| `Integer#gcd` / `#lcm` / `#gcdlcm` | MISSING | core.md | Number theory (also core). |
| `Integer#pow(b, mod)` (modular exp) | MISSING | core.md | Needed for crypto. |
| `Integer.sqrt(n)` (isqrt) | MISSING | core.md | |
| `Integer#digits(base)` / `#bit_length` / `#to_s(base)` | MISSING | core.md | Base conversion / bit ops. |

---

## 32. `matrix` — MISSING · no module (low priority)

Linear algebra. Rare in web/API apps; useful for graphics/ML. Low priority.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Matrix[[…],[…]]` / `Matrix.rows` / `Matrix.columns` / `Matrix.build(r,c){}` | MISSING | — | Construction. |
| `Matrix.identity(n)` / `.zero` / `.diagonal` / `.scalar` | MISSING | — | Special matrices. |
| `#+ #- #*` (matrix/scalar) / `#**` | MISSING | — | Arithmetic. |
| `#determinant/#det` / `#inverse/#inv` / `#transpose/#t` / `#rank` / `#trace/#tr` | MISSING | — | Operations. |
| `#[]` / `#row(i)` / `#column(j)` / `#row_count` / `#column_count` | MISSING | — | Access. |
| `#eigen` / `#eigensystem` / LUP decomposition | MISSING | — | Advanced. |
| `Vector[…]` (`#+ #- #* #inner_product/#dot #cross_product #magnitude/#norm #normalize`) | MISSING | — | Companion Vector type. |

---

## 33. `bigdecimal` — MISSING · no module

Arbitrary-precision decimal — required for money/financial math (float is
wrong there). (`Rational`/`Complex` are core numeric types — see [`core.md`](core.md).)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `BigDecimal(str[, precision])` / `"1.5".to_d` | MISSING | — | Construct from string (exact). |
| `#+ #- #* #/` / `#div(x, n)` / `#% / #modulo` | MISSING | — | Exact arithmetic with scale. |
| `#power(n[, prec])` / `#** / #sqrt(prec)` | MISSING | — | |
| `#round(n[, mode])` / `#truncate` / `#ceil` / `#floor` | MISSING | — | Rounding modes (banker's etc.). |
| `#to_s(fmt)` / `#to_f` / `#to_i` / `#to_r` | MISSING | — | Conversions. |
| `#precision` / `#exponent` / `#sign` / `#nan?` / `#infinite?` / `#finite?` / `#zero?` | MISSING | — | Introspection. |
| `BigMath` (`exp`, `log`, `PI`, `E`, `sin`, `cos`, `atan`, `sqrt`) | MISSING | — | Transcendental at precision. |

---

## 34. Marshal / `pp` / misc utility libs

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Marshal.dump(obj)` / `Marshal.load(str)` | COMPILE-TIME | — | Binary object serialization → per-type generated ser/de (serde-style). Arbitrary-graph runtime marshal is out of scope; typed codegen is the substitute. |
| `PP.pp(obj)` / `Kernel#pp` / `#pretty_inspect` | MISSING | — | Pretty-printer over `inspect` (core.md); buildable. |
| `PrettyPrint` (the engine) | MISSING | — | Wadler/Lindig algorithm; underlies `pp`. Low priority. |
| `Shellwords.escape(str)` / `.split(line)` / `String#shellsplit` | MISSING | — | Shell-safe quoting for `process`/`Command` (§Open3). |
| `Abbrev.abbrev(words)` | MISSING | — | Unique-prefix table; low value. |
| `English` (`$INPUT_RECORD_SEPARATOR` aliases) | OUT-OF-SCOPE | — | Global-variable aliases; not applicable. |
| `SecureRandom` (see §4) / `Digest` (§5) / `Base64` (§6) | — | — | Cross-refs. |

---

## 35. `open3` — PARTIAL · `library/std/process`

`process` provides `Command` with `.arg/.args/.env/.current_dir/.status/.output`
— `Command.output` already gives captured stdout+stderr+status (≈ `capture3`).
Missing: the streaming `popen3` (concurrent read/write of the child's pipes) and
pipelines.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Open3.capture3(*cmd, stdin_data:, binmode:)` → `[out, err, status]` | PARTIAL | `Command.output` → `Output{stdout, stderr, status}` | Have capture; no `stdin_data` feed. |
| `Open3.capture2(*cmd)` / `capture2e(*cmd)` | PARTIAL | `Command.output` | Subset of the above. |
| `Open3.popen3(*cmd){ |i,o,e,t| }` | MISSING | — | Streaming pipes (concurrent IO with the child). |
| `Open3.popen2(*cmd)` / `popen2e(*cmd)` | MISSING | — | |
| `Open3.pipeline(*cmds)` / `pipeline_r` / `pipeline_w` / `pipeline_rw` / `pipeline_start` | MISSING | — | Chained processes. |
| `Command.new(prog).arg/.args/.env/.current_dir` | DONE | `process.Command` | Builder present. |
| `Command#status` / `#output` | DONE | `process.Command` | `ExitStatus{code, success}` + `Output`. |
| `Process.spawn` / `Process.wait` / `Process.kill` / `system` / backtick | PARTIAL | `Command.status`/`.output`, `Process.exit` | High-level run present; low-level `spawn`/`wait`/signal control missing. |

---

## Out of scope (no runtime; tier-2 substitute noted)

These need a live metaobject runtime / GC / string-eval that Ruxen deliberately
omits. Each row names the compile-time substitute.

| Ruby API | Status | Substitute |
|----------|--------|-----------|
| `eval` / `instance_eval(str)` / `class_eval(str)` / `binding` | OUT-OF-SCOPE | Compile-time macros/codegen; templates compiled at build time (§13). |
| `ObjectSpace.each_object` / `count_objects` / `_id2ref` | OUT-OF-SCOPE | No heap walk; use explicit registries. |
| `WeakRef.new(obj)` / `WeakMap` | OUT-OF-SCOPE | No GC/weak refs; use ownership + explicit lifetimes, or an id→value map with manual eviction. |
| `GC.start` / `GC.stat` / `GC.disable` / `ObjectSpace::WeakMap` | OUT-OF-SCOPE | No GC (ownership-based); nothing to tune. |
| `TracePoint.new(:call){}` | OUT-OF-SCOPE | Compile-time instrumentation / explicit hooks. |
| `DRb` (distributed Ruby) | OUT-OF-SCOPE | Use `net`/`rondo` RPC over a real protocol. |
| `coverage` / `Ripper` / `RubyVM::*` | OUT-OF-SCOPE | Compiler-internal; not a user library. |
| `irb` / `reline` / `readline` (interactive REPL library) | OUT-OF-SCOPE | Ruxen ships a `repl` binary; not a linkable stdlib. |
| `did_you_mean` | OUT-OF-SCOPE | Compile-time diagnostic (compiler already suggests). |
| `rbconfig` (`RbConfig::CONFIG`) | PARTIAL | `env` covers runtime host info (`cpu_count`, `vars`, `current_dir`). |
| `set` `SortedSet`, `matrix` eigen, `bigdecimal` `BigMath` | (see rows) | Tracked above; low priority. |

---

## Priority backlog — missing stdlib, ranked by app value

Ranked for the North-Star goal (build a real web/API + DB app in Ruxen). A row's
rank reflects both value and dependency order (crypto/encoding unblock net-http;
date/time unblocks records + serialization).

**P0 — a real app is blocked without these:**
1. **`date`/`time` (§3)** — civil `Time` + `Date`, `strftime`/`strptime`/parse,
   ISO-8601 + `rfc2822`/`httpdate` emitters, arithmetic. Records, APIs, logs,
   cookies all need it. Biggest single gap. Unblocks logger timestamps, http
   dates, uuid_v7.
2. **`base64` (§6)** — tiny, self-contained; dependency of securerandom,
   JWT, Basic-Auth, data URIs.
3. **`digest` + HMAC (§5, §8)** — SHA-256/SHA-1/MD5 one-shot + streaming, and
   `HMAC` (webhook signatures, JWT HS256, token hashing). Needs the hex encoder
   shared with securerandom.
4. **`securerandom` conveniences (§4)** — `uuid`, `hex`, `base64`,
   `random_number`, `alphanumeric`. **`uuid` is required for primary keys.**
5. **`uri` (§7)** — parse + `encode_www_form`/component codecs. Blocks net/http
   query/form bodies and routing.

**P1 — needed for a complete networked app:**
6. **`net/http` full client (§9)** — verbs, headers map, request/response
   objects, form bodies; then **HTTPS** (needs openssl TLS §8).
7. **`openssl` TLS + Cipher (§8)** — `SSLContext`/`SSLSocket` (gates HTTPS),
   symmetric `Cipher` (encrypted cookies/sessions), KDFs (pbkdf2/scrypt/hkdf).
8. **`logger` (§24)** — every service needs leveled logging.
9. **`cgi` escaping (§10)** — `escapeHTML` (XSS-safe views, feeds `erb`),
   `escape`/`unescape`.
10. **`csv` (§11)** — import/export; the standard data-exchange format.
11. **`yaml` safe subset (§12)** — config/fixtures (`safe_load` + `dump`).
12. **`json` option gaps + `to_json` derive (§1)** — object↔JSON mapping
    (compile-time serializer) so structs round-trip without hand-built nodes.

**P2 — ergonomics + completeness:**
13. **`set` completion (§2)** — subset/superset/`^`/merge + Enumerable mixin.
14. **`Struct`/`Data` codegen (§14)** + **`forwardable` (§15)** + **`singleton`
    (§16)** — the compile-time-synthesis batch (shared `derive`/macro machinery,
    also used by ostruct/delegate).
15. **`stringio` (§17)** — in-memory IO; unblocks clean csv/erb/json string
    output.
16. **`pathname` (§18)** + **`fileutils` gaps (§19: `cp_r`, `touch`, `Dir.glob`,
    `Find`)** — object path API + recursive fs ops over the existing `fs`
    primitives.
17. **`tempfile` (§20)** — temp files/dirs (uploads, atomic writes, tests).
18. **`optparse` (§25)** — CLI tooling.
19. **thread/queue ergonomics (§27)** — typed `Thread.new`/`#value`, `Queue`/
    `SizedQueue`, `ConditionVariable`; **`monitor` (§29)** reentrant lock.
20. **`erb` compile-time templates (§13)** — view layer (pairs with gems-web).

**P3 — niche / low frequency:**
21. `open3` streaming (§35), `benchmark` `bm`/`Tms` (§26), `timeout` (§30),
    `observer` (§23), `delegate` (§22).
22. `prime`/number-theory (§31), `bigdecimal` (§33) — the latter matters
    specifically for money/financial apps (rank up if that's the target).
23. `matrix` (§32), `pp`/`PrettyPrint`, `shellwords`, `abbrev` (§34).

**Deliberately not built** (see Out-of-scope): `eval`/`binding`, `ObjectSpace`,
`WeakRef`, `GC`, `TracePoint`, `DRb`, `irb`/`readline` library, `Marshal`
runtime graph (compile-time typed ser/de substitutes it).
