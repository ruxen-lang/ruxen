# Ruby standard-library parity — audit matrix

Authoritative, no-skip backlog for **Ruby's bundled standard library** (the
`require`-able libraries that ship with CRuby/MRI, *not* core classes — those
live in [`core.md`](core.md) — and *not* third-party gems — those live in
`gems-*.md`). Every library gets a table; every real public API gets a row.

Read [`README.md`](README.md) first for the tier framework. Recap of the status
values used in every table below:

| Status | Meaning |
|--------|---------|
| **DONE**, tagged [implemented] | Full parity exists in Ruxen today (module named). |
| **PARTIAL**, tagged [implemented]; the note names the gap | Some parity exists; the gap is spelled out in the Note. |
| **MISSING**, tagged [planned](../../TODO.md#t-parity-stdlib) (or the more specific tracker item the row links) | Not implemented. The row scopes what a parity module must provide. |
| **COMPILE-TIME**, tagged [planned](../../TODO.md#t-parity-compile-time) unless the row says it already works | Ruby does it with load-time/runtime metaprogramming; Ruxen re-expresses it with compile-time synthesis (macro/mixin/codegen). The row names the mechanism. |
| **OUT-OF-SCOPE**, tagged [retired]: Ruxen will not build it | Needs a live runtime Ruxen deliberately lacks (`eval`/`binding`/reflection/GC). The row names the tier-2 substitute. |

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
| `JSON.parse(str)` | [implemented] DONE | `JSON.parse` → `Result[Json, JsonError]` | Ruby raises; Ruxen returns `Result`. |
| `JSON.parse(str, symbolize_names: true)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | No `Symbol` distinction; keys stay `String`. Low value in a typed lang. |
| `JSON.parse(str, max_nesting: n)` | [implemented] PARTIAL | `JsonError::DepthLimit` | Fixed depth limit; not caller-configurable. |
| `JSON.parse(str, allow_nan: true)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | NaN/Infinity extension not accepted. |
| `JSON.parse(str, create_additions: true)` | [retired] OUT-OF-SCOPE | — | Needs runtime class dispatch on `json_class`; use a compile-time deserializer. |
| `JSON.parse!(str)` | [implemented] PARTIAL | `JSON.parse_strict` | Strict variant exists; semantics differ (not "no limits"). |
| `JSON.generate(obj)` | [implemented] DONE | `JSON.stringify(&Json)` → `Result[String]` | Operates on a `Json` node, not arbitrary objects. |
| `JSON.generate(obj, opts)` (indent/space/space_before/object_nl/array_nl) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | No fine-grained whitespace options; only pretty vs compact. |
| `JSON.pretty_generate(obj)` | [implemented] DONE | `JSON.stringify_pretty(&Json, indent: Int)` | Indent width is a param. |
| `JSON.dump(obj[, io[, limit]])` | [planned](../../TODO.md#t-parity-stdlib) MISSING | alias of `stringify` | Add `JSON.dump` alias; `io`/`limit` args need `StringIO`/`File` sink. |
| `JSON.load(source[, proc[, opts]])` | [planned](../../TODO.md#t-parity-stdlib) MISSING | alias of `parse` | Add `JSON.load`; `proc` transform + `create_additions` out of scope. |
| `JSON.load_file(path)` / `JSON.dump_file` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Compose `fs.read_to_string` + `parse`; add convenience. |
| `Object#to_json` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Generate `to_json` per struct/enum from fields (compile-time codegen). |
| `String#to_json`, `Integer#to_json`, … (core `to_json`) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Same codegen; primitives map to `JSON.make_*`. |
| `JSON[str]` / `JSON[obj]` (round-trip shortcut) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Sugar over parse/generate. |
| `JSON.generator` / `JSON.parser` / `JSON.create_id` | [retired] OUT-OF-SCOPE | — | Pluggable backend selection; not applicable to a single C backend. |
| Node inspection: `.kind`, `.is_*`, `.as_*`, `.array_len/get`, `.object_len/has/get` | [implemented] DONE | `JSON.*` facade | Richer than Ruby (Ruby returns plain `Hash`/`Array`); Ruxen keeps a typed node. |
| Builders: `null_value/bool/int/float/string/array/object/empty_*` | [implemented] DONE | `JSON.*` | No Ruby analog; used to construct nodes for generation. |

---

## 2. `set` — PARTIAL · `library/std/set`

`Set[T]` exists with the core membership + boolean-algebra operations. Gaps are
the mutating-with-answer variants (`add?`/`delete?`), the comparison predicates
(subset/superset/disjoint), symmetric difference, `merge`/`subtract`, and the
whole `Enumerable` surface (Ruby's `Set` includes `Enumerable`).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Set.new(enum=nil)` | [implemented] PARTIAL | `Set.new` (empty) / `Set.with_capacity` | No "from enumerable" ctor; add `Set.from(Array[T])`. |
| `Set[a, b, …]` literal | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Needs a variadic/array constructor sugar. |
| `#add(o)` / `#<<` | [implemented] PARTIAL | `#insert(item)` | Rename/alias to `add`/`<<` for Ruby ergonomics. |
| `#add?(o)` → self or nil | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Insert returning whether it was new. |
| `#delete(o)` | [implemented] PARTIAL | `#remove(item) -> Bool` | Ruby `delete` returns self; `delete?` returns nil/self. Map. |
| `#delete?(o)` | [implemented] PARTIAL | `#remove` returns Bool | Semantics close; align return. |
| `#delete_if` / `#reject!` / `#keep_if` / `#select!` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Needs block predicate + in-place. |
| `#include?` / `#member?` / `#===` | [implemented] PARTIAL | `#include?` | Add `member?`/`===` aliases. |
| `#size` / `#length` / `#count` | [implemented] PARTIAL | `#size` | Add `length` alias; `count` from Enumerable. |
| `#empty?` | [implemented] DONE | `#empty?` | |
| `#clear` | [implemented] DONE | `#clear` | |
| `#each` | [implemented] DONE | free-fn `each` | Present; wire into an Enumerable mixin. |
| `#to_a` | [implemented] DONE | `#to_a` → `Array[&T]` | |
| `#to_set` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Trivial identity/convert. |
| `#\|` / `#union` / `#+` / `#merge` | [implemented] PARTIAL | `#union` | Add operator aliases + `merge` (mutating union). |
| `#&` / `#intersection` | [implemented] PARTIAL | `#intersection` | Add `&` alias. |
| `#-` / `#difference` / `#subtract` | [implemented] PARTIAL | `#difference` | Add `-` alias + `subtract` (mutating). |
| `#^` (symmetric difference) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `(a\|b) - (a&b)`. |
| `#subset?` / `#<=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Boolean predicate. |
| `#proper_subset?` / `#<` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#superset?` / `#>=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#proper_superset?` / `#>` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#disjoint?` / `#intersect?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#==` / `#hash` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Value equality across two sets. |
| `#replace(enum)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#classify {block}` → Hash | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Enumerable-style grouping. |
| `#divide {block}` → Set of Sets | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Tarjan partition; low priority. |
| `#flatten` / `#flatten!` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Set-of-sets flatten. |
| `Enumerable` methods (`map`/`select`/`reduce`/`min`/`max`/`sum`/`group_by`/`sort`/…) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Include the Enumerable mixin (see §21) over `each`. |
| `SortedSet` | [retired] OUT-OF-SCOPE | — | Removed from Ruby stdlib in 3.0 (now the `sorted_set` gem). Track under gems if demanded. |

---

## 3. `date` / `time` — MISSING (large) · `library/std/time` has monotonic clock only

**The biggest single stdlib gap.** Ruxen's `time` module provides only
`Instant`/`Duration` (a *monotonic* clock) and `Time.unix_ns` (a raw
wall-clock nanosecond count). There is **no calendar `Date`, no `DateTime`, no
civil `Time`, no `strftime`/`strptime`, no timezone handling, no calendar
arithmetic**. A parity module must build all of it. This is P0 for any real app
(records have `created_at`, APIs emit ISO-8601).

### 3a. `Time` (core class; `strftime`/parse from the `time` stdlib)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Time.now` | [planned](../../TODO.md#t-stdlib-shapes) MISSING | `Time.unix_ns` (raw ns only) | Need a civil `Time` value carrying y/m/d/h/m/s/ns + offset. |
| `Time.at(seconds[, subsec])` | [planned](../../TODO.md#t-parity-time) MISSING | — | Construct from epoch. |
| `Time.new(y,m,d,h,mi,s, tz)` / `Time.local` / `Time.mktime` | [planned](../../TODO.md#t-parity-time) MISSING | — | Local-zone construction. |
| `Time.utc(...)` / `Time.gm(...)` | [planned](../../TODO.md#t-parity-time) MISSING | — | UTC construction. |
| `Time.parse(str)` (time stdlib) | [planned](../../TODO.md#t-parity-time) MISSING | — | Heuristic parse; key for ingest. |
| `Time.strptime(str, fmt)` (time stdlib) | [planned](../../TODO.md#t-parity-time) MISSING | — | Format-directed parse. |
| `Time.iso8601(str)` / `Time.xmlschema` | [planned](../../TODO.md#t-parity-time) MISSING | — | ISO-8601 ingest. |
| `#year #month/#mon #day/#mday #hour #min #sec #usec #nsec #subsec` | [planned](../../TODO.md#t-parity-time) MISSING | — | Field accessors. |
| `#wday #yday #monday?..#sunday?` | [planned](../../TODO.md#t-parity-time) MISSING | — | Weekday accessors. |
| `#to_i #to_f #to_r` | [planned](../../TODO.md#t-parity-time) MISSING | — | Epoch conversions. |
| `#+ (sec)` / `#- (sec or Time)` | [planned](../../TODO.md#t-parity-time) MISSING | — | Arithmetic → `Time` or `Float` seconds. |
| `#<=> #== #eql? #hash` | [planned](../../TODO.md#t-parity-time) MISSING | — | Comparable ordering. |
| `#strftime(fmt)` | [planned](../../TODO.md#t-parity-time) MISSING | — | The formatting workhorse (`%Y-%m-%d %H:%M:%S %z …`). |
| `#iso8601` / `#xmlschema` / `#rfc2822` / `#httpdate` / `#rfc3339` | [planned](../../TODO.md#t-parity-time) MISSING | — | Canonical string emitters. |
| `#utc? #utc/#getutc #localtime/#getlocal #zone #utc_offset/#gmt_offset` | [planned](../../TODO.md#t-parity-time) MISSING | — | Zone handling. |
| `#dst?` | [planned](../../TODO.md#t-parity-time) MISSING | — | DST flag (needs tz database). |
| `#round(n) #ceil(n) #floor(n)` | [planned](../../TODO.md#t-parity-time) MISSING | — | Subsecond rounding. |
| `Time::Comparable` (`between?`, `clamp`) | [planned](../../TODO.md#t-parity-time) MISSING | — | Via Comparable mixin (§21). |

### 3b. `Date` (the `date` stdlib)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Date.new(y,m,d)` / `Date.civil` | [planned](../../TODO.md#t-parity-time) MISSING | — | Proleptic Gregorian civil date. |
| `Date.today` | [planned](../../TODO.md#t-parity-time) MISSING | — | |
| `Date.parse(str)` / `Date.iso8601` / `Date.strptime(str, fmt)` | [planned](../../TODO.md#t-parity-time) MISSING | — | Parsing. |
| `Date.jd(n)` / `Date.ordinal` / `Date.commercial` | [planned](../../TODO.md#t-parity-time) MISSING | — | Alt calendars/ordinals. |
| `#year #month/#mon #day/#mday #wday #yday #cwday #cweek #cwyear` | [planned](../../TODO.md#t-parity-time) MISSING | — | Field accessors. |
| `#leap? #julian? #gregorian?` | [planned](../../TODO.md#t-parity-time) MISSING | — | Calendar predicates. |
| `#+ (days) #- (days or Date→Rational) #<< (months) #>> (months)` | [planned](../../TODO.md#t-parity-time) MISSING | — | Arithmetic. |
| `#next_day #prev_day #next_month #prev_month #next_year #prev_year #succ/#next` | [planned](../../TODO.md#t-parity-time) MISSING | — | Stepping. |
| `#step(limit, step) #upto #downto` | [planned](../../TODO.md#t-parity-time) MISSING | — | Iteration. |
| `#<=> #== #eql?` | [planned](../../TODO.md#t-parity-time) MISSING | — | Comparable. |
| `#strftime #iso8601 #rfc3339 #httpdate #jd #mjd #ajd` | [planned](../../TODO.md#t-parity-time) MISSING | — | Formatting/serials. |
| `#to_time #to_date #to_datetime` | [planned](../../TODO.md#t-parity-time) MISSING | — | Conversions. |
| `#monday?..#sunday?` | [planned](../../TODO.md#t-parity-time) MISSING | — | Weekday predicates. |

### 3c. `DateTime` (the `date` stdlib; `Date` subclass with time-of-day)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `DateTime.new(y,m,d,h,mi,s,offset)` / `.now` / `.parse` / `.strptime` / `.iso8601` | [planned](../../TODO.md#t-parity-time) MISSING | — | Construction/parse with offset. |
| `#hour #min/#minute #sec/#second #sec_fraction #zone #offset` | [planned](../../TODO.md#t-parity-time) MISSING | — | Time-of-day + zone. |
| `#new_offset(off)` | [planned](../../TODO.md#t-parity-time) MISSING | — | Re-zone. |
| `#to_time #to_date` | [planned](../../TODO.md#t-parity-time) MISSING | — | Conversions. |
| `#strftime #iso8601 #rfc3339 #httpdate` | [planned](../../TODO.md#t-parity-time) MISSING | — | Formatting. |

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
| `SecureRandom.random_bytes(n=16)` | [implemented] DONE | `Rand.random_bytes(n)` → `Result[Array[Int]]` | |
| `SecureRandom.bytes(n)` | [implemented] DONE | `Rand.random_bytes` | Alias. |
| `SecureRandom.hex(n=16)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Bytes → lowercase hex (needs a hex encoder; shared with `digest`). |
| `SecureRandom.base64(n=16)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Needs `base64` (§7). |
| `SecureRandom.urlsafe_base64(n=16, padding=false)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Needs `base64.urlsafe_*`. |
| `SecureRandom.uuid` / `uuid_v4` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P0.** 16 random bytes → v4 UUID string. |
| `SecureRandom.uuid_v7` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Time-ordered UUID (Ruby 3.3+); needs wall clock (§3). |
| `SecureRandom.random_number(n=0)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `n` Integer → `[0,n)`; `n` Float → `[0.0,n)`; `0` → `[0.0,1.0)`. |
| `SecureRandom.alphanumeric(n=16, chars:)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Random `[A-Za-z0-9]` string. |
| `SecureRandom.rand` / `SecureRandom.choose` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `rand` alias of `random_number`; `choose` (3.3+) picks from a charset. |
| `SecureRandom.gen_random(n)` | [implemented] DONE | `Rand.random_bytes` | Internal primitive; same thing. |
| Non-secure `Random` / `Kernel#rand` / `Kernel#srand` / `Array#sample` / `#shuffle` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Core PRNG (seedable, reproducible) — cross-ref `core.md`; distinct from CSPRNG. |

---

## 5. `digest` — MISSING · no module

Cryptographic + checksum hashing. Zero coverage today. Needed for
password/token hashing, ETags, cache keys, HMAC signing (webhooks, JWT). A
parity module wraps a C hash backend and exposes both the one-shot class methods
and the streaming instance API.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Digest::MD5.hexdigest(s)` / `.digest(s)` / `.base64digest(s)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | MD5 (non-crypto uses: ETag/cache key). |
| `Digest::SHA1.hexdigest/.digest/.base64digest` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `Digest::SHA256.hexdigest/.digest/.base64digest` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P0** (tokens, signatures). |
| `Digest::SHA384` / `Digest::SHA512` / `Digest::SHA2.new(bitlen)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `Digest::RMD160` / `Digest::BubbleBabble` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Low value. |
| `Digest::CLASS.file(path)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Hash a file (stream from `fs`/`io`). |
| `#new` / `#update(s)` / `#<<` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Streaming/incremental hashing. |
| `#digest` / `#hexdigest` / `#base64digest` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Finalize (raw / hex / base64). |
| `#reset` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Reuse the context. |
| `#digest_length` / `#block_length` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Algorithm metadata. |
| `#==` (constant-time compare) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Digest equality. |
| `Digest.hexencode(str)` / `Digest.bubblebabble` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Byte→hex helper (shared with SecureRandom.hex). |
| `OpenSSL::HMAC.hexdigest(digest, key, data)` / `.digest` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P0** — HMAC (webhook signatures, JWT HS256). Lives in `openssl` (§8) in Ruby but pair it here. |

---

## 6. `base64` — DONE · `library/std/base64`

The `base64` package exists (`class Base64`, FFI). Full 6-method surface.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Base64.encode64(bin)` | [implemented] DONE | `Base64.encode64` | MIME variant (line-wrap semantics unverified). |
| `Base64.decode64(str)` | [implemented] DONE | `Base64.decode64` | |
| `Base64.strict_encode64(bin)` | [implemented] DONE | `Base64.strict_encode64` | |
| `Base64.strict_decode64(str)` | [implemented] DONE | `Base64.strict_decode64` | |
| `Base64.urlsafe_encode64(bin, padding: true)` | [implemented] DONE | `Base64.urlsafe_encode64` | |
| `Base64.urlsafe_decode64(str)` | [implemented] DONE | `Base64.urlsafe_decode64` | |

---

## 7. `uri` — MISSING · no module

URL parsing/construction + `www-form` encoding. Needed by `net/http` (§9), form
bodies, query strings, OAuth, routing. `CGI.escape`-style component encoding
overlaps with `cgi` (§10) — share one percent-codec.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `URI.parse(str)` / `URI(str)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Returns a typed URI (HTTP/HTTPS/Generic). Ruxen: one `Uri` struct with a `scheme`. |
| `URI.join(base, *rel)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Resolve relative refs (RFC-3986). |
| `URI.encode_www_form(enum)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `[[k,v],…]` → `a=1&b=2` (P0 for form/query bodies). |
| `URI.decode_www_form(str)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Inverse → `[[k,v],…]`. |
| `URI.encode_www_form_component(s)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Percent-encode one component. |
| `URI.decode_www_form_component(s)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Percent-decode one component. |
| `#scheme #userinfo #user #password #host #hostname #port #path #query #fragment #opaque` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Component accessors (+ setters in Ruby). |
| `#to_s` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Reassemble. |
| `#merge(rel)` / `#+` / `#route_to` / `#relative?` / `#absolute?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Relative-reference algebra. |
| `#normalize` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Canonicalize scheme/host case, default port. |
| `#request_uri` (HTTP) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `path?query` for the request line. |
| `#select(*components)` / `#component` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Introspection; low value. |
| `URI.extract` / `URI.regexp` | [retired] OUT-OF-SCOPE | — | Deprecated in Ruby; skip. |
| `URI::DEFAULT_PARSER` / `RFC2396_Parser` / `RFC3986_Parser` | [retired] OUT-OF-SCOPE | — | Pluggable parser objects; a single RFC-3986 parser suffices. |

---

## 8. `openssl` — MISSING · no module (TLS gates HTTPS)

Huge surface. The load-bearing pieces for apps are **TLS** (blocks HTTPS in
`net/http`), **HMAC** (§5), **symmetric ciphers** (session/cookie encryption),
and **digests** (§5). PKI/X.509 is lower priority. All of this binds to a TLS/
crypto backend via FFI (rustls + ring, or system OpenSSL) — a runtime decision,
not a compile-time-synthesis one.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `OpenSSL::SSL::SSLContext.new` (`verify_mode=`, `ca_file=`, `cert=`, `key=`, `min_version=`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** — TLS config; required for HTTPS client + server. |
| `OpenSSL::SSL::SSLSocket.new(tcp, ctx)` (`connect`, `accept`, `read`, `write`, `peer_cert`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** — wraps `net.TcpStream` in TLS. |
| `OpenSSL::HMAC.hexdigest/.digest(digest, key, data)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P0** (see §5). |
| `OpenSSL::Digest.new(name)` / `OpenSSL::Digest::SHA256` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Same backend as `digest` (§5). |
| `OpenSSL::Cipher.new("aes-256-gcm")` (`encrypt`/`decrypt`, `key=`, `iv=`, `auth_data=`, `auth_tag`, `update`, `final`, `random_key`, `random_iv`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** — encrypted cookies/sessions/secrets. |
| `OpenSSL::PKey::RSA` / `EC` / `DSA` (`generate`, `sign`, `verify`, `public_encrypt`, `private_decrypt`, `to_pem`, `from_pem`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Asymmetric crypto (JWT RS256/ES256, key exchange). |
| `OpenSSL::X509::Certificate` / `Name` / `Store` / `Request` / `CRL` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Cert parsing/validation/issuance. |
| `OpenSSL::PKCS7` / `OpenSSL::PKCS5.pbkdf2_hmac` / `OpenSSL::KDF` (`scrypt`, `hkdf`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Password KDFs (scrypt/pbkdf2/hkdf) for auth. |
| `OpenSSL::Random.random_bytes` | [implemented] DONE (equiv) | `Rand.random_bytes` | Same CSPRNG need; unify. |
| `OpenSSL::BN` (bignum) | [retired] OUT-OF-SCOPE | — | Arbitrary-precision integers; cross-ref `bigdecimal`/core Integer. |

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
| `Net::HTTP.get(uri)` / `.get(host, path, port)` | [implemented] PARTIAL | `Http.get(host, port, path)` | Exists for plain HTTP; no URI overload, no TLS. |
| `Net::HTTP.get_response(uri)` → response | [implemented] PARTIAL | returns `Option[HttpResponse]` | Response carries `status`+`body`; no headers map. |
| `Net::HTTP.get_print(uri)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Convenience. |
| `Net::HTTP.post(uri, data, header)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** POST body + headers. |
| `Net::HTTP.post_form(uri, params)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `www-form` POST (needs `uri.encode_www_form`). |
| `Net::HTTP.put/patch/delete(uri, …)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Other verbs. |
| `Net::HTTP.start(host, port, opts){\|http\| }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Persistent connection scope. |
| `Net::HTTP.new(host, port)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Configurable client object (below). |

### 9b. `Net::HTTP` instance (connection object)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `#start` / `#finish` / `#started?` / `#active?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Connection lifecycle (keep-alive). |
| `#get/#post/#put/#patch/#delete/#head/#options(path, …)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Verb methods on the connection. |
| `#request(req[, body])` / `#request_get` / `#request_post` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Send a prebuilt request object. |
| `#use_ssl=` / `#verify_mode=` / `#ca_file=` / `#cert=` / `#key=` / `#ssl_version=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** HTTPS (needs `openssl` §8). |
| `#read_timeout=` / `#open_timeout=` / `#write_timeout=` / `#keep_alive_timeout=` | [implemented] PARTIAL | `net.TcpStream.set_*_timeout` exists | Plumb through to the client. |
| `#set_debug_output(io)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Wire logging. |
| `#proxy_*` / `Net::HTTP.Proxy` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Proxy support; lower priority. |
| `#address` / `#port` / `#ipaddr` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Introspection. |

### 9c. Request classes + response

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Net::HTTP::Get/Post/Put/Patch/Delete/Head/Options.new(path, headers)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Request builders. |
| `req.body=` / `req.body_stream=` / `req.set_form_data(hash)` / `req.set_form(...)` (multipart) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Request body + form encoding. |
| `req["Header"]=` / `req.add_field` / `req.basic_auth(u,p)` / `req.content_type=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Header setters (Basic-Auth needs `base64` §6). |
| `res.code` / `res.message` / `res.body` / `res.read_body` | [implemented] PARTIAL | `HttpResponse.status`/`.body`/`.ok?` | Have status+body; no streaming read. |
| `res["Header"]` / `res.each_header` / `res.get_fields` / `res.content_type` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** response header access (parse into a map). |
| `res.value` (raise on non-2xx) | [implemented] PARTIAL | `HttpResponse.ok?` | Have the predicate; add raising variant. |
| `res.kind_of?(Net::HTTPSuccess/Redirection/ClientError/ServerError)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Status-class taxonomy. |
| Automatic redirect following | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | App must loop today; add opt-in follow. |
| Chunked transfer-encoding / gzip decoding | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Current parser assumes `Content-Length` + `Connection: close`. |

---

## 10. `cgi` (escaping helpers) — MISSING · no module

Only the escaping utilities are in scope (the full `CGI` request object is
rondo's job). Share the percent-codec with `uri` (§7).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `CGI.escape(str)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `application/x-www-form-urlencoded` (space→`+`). |
| `CGI.unescape(str)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Inverse. |
| `CGI.escapeHTML(str)` / `CGI.escape_html` / `h` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** — `&<>"'` → entities (XSS-safe view output; needed by `erb` §13). |
| `CGI.unescapeHTML(str)` / `CGI.unescape_html` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Inverse. |
| `CGI.escapeURIComponent` / `CGI.unescapeURIComponent` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | RFC-3986 component (space→`%20`). |
| `CGI.escapeElement` / `CGI.unescapeElement` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Selective element escaping; low value. |
| `CGI.parse(query_string)` → `Hash` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Overlaps `URI.decode_www_form`. |
| `CGI.new` (params/cookies/headers request object) | [retired] OUT-OF-SCOPE | rondo | Server request handling lives in the web framework. |

---

## 11. `csv` — MISSING · no module

Tabular import/export — the standard "load a spreadsheet / export a report"
path. P1 for data apps. Build on `stringio` (§17) for the in-memory case.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `CSV.parse(str, **opts)` → `Array[Array]` (or rows) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Full-string parse. |
| `CSV.parse_line(str, **opts)` → one row | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Single row. |
| `CSV.foreach(path, **opts){\|row\| }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Streaming file read (via `io`). |
| `CSV.read(path, **opts)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Read whole file. |
| `CSV.open(path, mode, **opts){\|csv\| }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | File handle wrapper. |
| `CSV.generate(**opts){\|csv\| csv << row }` → String | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Build CSV text. |
| `CSV.generate_line(row, **opts)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | One row → String (quoting rules). |
| `CSV.new(io_or_string, **opts)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Reader/writer object. |
| `#shift` / `#read` / `#each` / `#<<` / `#to_a` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Row iteration + append. |
| Options: `headers:`, `col_sep:`, `row_sep:`, `quote_char:`, `skip_blanks:`, `return_headers:`, `write_headers:`, `liberal_parsing:` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | At minimum `headers`, `col_sep`, `quote_char`. |
| Options: `converters:` / `header_converters:` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Typed field conversion; needs a converter registry. |
| `CSV::Row` (`#[]`, `#[]=`, `#field`, `#headers`, `#to_h`, `#to_a`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Header-addressed row (P1 ergonomics). |
| `CSV::Table` (`#[]`, `#by_col`, `#by_row`, `#to_a`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Table view; lower priority. |

---

## 12. `yaml` (Psych) — MISSING · no module

Config-file format. `safe_load` of scalars/maps/sequences + `dump` covers the
90% (config, fixtures). Full YAML (anchors/aliases/custom tags/merge keys) is a
large spec — scope a subset first.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `YAML.safe_load(str, permitted_classes:, aliases:)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | **P1** — default `load` since Ruby 3.1. Scalars/Hash/Array/nil/bool/int/float. |
| `YAML.load(str)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Alias of `safe_load` in modern Ruby. |
| `YAML.load_file(path)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Config read (via `fs`). |
| `YAML.unsafe_load(str)` | [retired] OUT-OF-SCOPE | — | Instantiates arbitrary classes at runtime; use a compile-time typed loader instead. |
| `YAML.dump(obj[, io])` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Emit YAML. Arbitrary-object dump → COMPILE-TIME serializer (like `to_json`). |
| `YAML.load_stream` / `YAML.dump_stream` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Multi-document. |
| `YAML.parse(str)` / `YAML.parse_stream` → AST | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Node tree; lower priority. |
| Anchors/aliases (`&a` / `*a`), merge keys (`<<`), custom tags (`!ruby/object`) | [implemented] PARTIAL (anchors, merge keys) / [retired] OUT-OF-SCOPE (`!ruby/object` tags) | — | Anchors/merge = PARTIAL (parseable); `!ruby/object` tags = out of scope (no runtime class table). |

---

## 13. `erb` — COMPILE-TIME · no module

Templating. Ruby compiles a template string to Ruby code, then `eval`s it
against a `Binding`. Ruxen's design (README tier 2): **compile the template at
build time into a Ruxen function** that takes a typed context struct and returns
a `String`; `<%= expr %>`/`<% code %>` become emitted expressions/statements. No
runtime `binding`. Cross-ref `gems-web.md` (view layer).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `ERB.new(template, trim_mode:, eoutvar:)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Template → generated render fn. `trim_mode` (`-`/`<>`/`%`) = codegen flag. |
| `#result(binding)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Replaced by `render(ctx)` over a typed context; no `Binding`. |
| `#result_with_hash(hash)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Context passed as a struct/hash of typed locals. |
| `#def_method` / `#def_module` / `#def_class` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Template already compiles to a named fn — this is the default shape. |
| `ERB::Util.html_escape` / `h(s)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Delegates to `CGI.escapeHTML` (§10). |
| `ERB::Util.url_encode` / `u(s)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Delegates to the percent-codec (§7/§10). |
| Runtime template from an unknown string at runtime | [retired] OUT-OF-SCOPE | — | Needs `eval`; only build-time-known templates compile. Document the substitute (embed the template file). |

---

## 14. `struct` / `ostruct` — COMPILE-TIME · no module

The canonical compile-time win. `Struct.new(...)` generating a class is exactly
tier 2: emit a Ruxen struct + accessors. `OpenStruct`'s runtime-added fields are
the awkward case (no runtime field insertion) — substitute a compile-time
`Struct`/`Data` or a `Hash`.

### 14a. `Struct` (core-ish, but the `.new`-generates-a-class metaprogramming lands here)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Struct.new(:a, :b) { … }` → class | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | macro/codegen | Generate a nominal struct with the named fields + a block of methods. |
| `Struct.new(:a, keyword_init: true)` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | — | Keyword ctor variant. |
| accessors `#a` / `#a=` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | — | Generated getter/setter per member. |
| `#[](name_or_idx)` / `#[]=` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | — | By-name/index access (index needs a generated match). |
| `#members` / `.members` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | — | Static list of field names. |
| `#to_a` / `#values` / `#to_h` / `#values_at` / `#deconstruct` / `#deconstruct_keys` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | — | Generated conversions (pattern-match support). |
| `#each` / `#each_pair` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | — | Generated iteration over fields. |
| `#== #eql? #hash #dig #size/#length` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | — | Generated structural equality/dig. |
| `Data.define(:a, :b)` (Ruby 3.2+ immutable) → `#with`, `#to_h`, `#deconstruct` | [planned](../../TODO.md#t-data-define) COMPILE-TIME | — | Immutable variant; maps cleanly to a value struct + `with` builder. |

### 14b. `ostruct` (`OpenStruct`)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `OpenStruct.new(hash)` with fields fixed at construction | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | If the field set is a literal known at compile time, generate a struct. |
| `os.foo` / `os.foo = x` (fields added *after* construction) | [retired] OUT-OF-SCOPE | — | Runtime field insertion needs an open object; substitute a `Hash[String, Value]` or a compile-time struct. |
| `#[]` / `#[]=` / `#to_h` / `#each_pair` / `#dig` / `#respond_to?` / `#delete_field` | [implemented] PARTIAL | — | Provide over the `Hash` substitute; `respond_to?` is compile-time-known. |
| `#== ` / marshal | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Structural over the backing map. |

---

## 15. `forwardable` — COMPILE-TIME · no module

Method delegation. Ruby installs forwarding methods at load time via
`def_delegator`. Ruxen generates them at compile time (the accessor + target
method name are literals).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `extend Forwardable` | [planned](../../TODO.md#t-extend) COMPILE-TIME | — | Marker for the codegen; no runtime module. |
| `def_delegator(accessor, method, ali = method)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Generate `def ali(*a) = accessor.method(*a)`. |
| `def_delegators(accessor, *methods)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Batch of the above. |
| `delegate(method => accessor, …)` / `delegate([m1,m2] => accessor)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Hash form. |
| `instance_delegate` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Same generation. |
| `SingleForwardable` / `single_delegate` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Delegates on the singleton/module; same mechanism. |

---

## 16. `singleton` — COMPILE-TIME · no module

`include Singleton` → a lazily-created, memoized `.instance` with a private
`new`. Ruxen generates a module-level accessor (with a `sync`-guarded init for
thread safety).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `include Singleton` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Generate the instance holder + accessor; make `new` private. |
| `.instance` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Memoized singleton getter (thread-safe init via `sync`/atomic). |
| private `.new`, disabled `#dup`/`#clone` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Compile-time visibility + deleted copy path. |
| `._load` / `#_dump` (marshal hooks) | [retired] OUT-OF-SCOPE | — | Marshal integration; skip. |

---

## 17. `stringio` — MISSING · no module

In-memory IO with the `IO` interface. A dependency of `csv`/`erb`/`json` output
and of any code that wants a `File`-shaped sink without touching disk. P2 but
cheap and widely useful.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `StringIO.new(str = "", mode = "r+")` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Backing buffer + cursor. |
| `#read([len])` / `#gets` / `#readline` / `#readlines` / `#each_line` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Reading. |
| `#getc` / `#ungetc` / `#getbyte` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Char/byte reads. |
| `#write(s)` / `#<<` / `#puts` / `#print` / `#printf` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Writing. |
| `#string` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Get the accumulated buffer. |
| `#rewind` / `#seek(off, whence)` / `#pos` / `#pos=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Cursor control. |
| `#eof?` / `#truncate(n)` / `#size` / `#close` / `#closed?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | State. |

---

## 18. `pathname` — PARTIAL · `library/std/path` (static string ops only)

Ruxen's `path` module is pure *string* manipulation (`join`, `parent`,
`file_name`, `extname`, `is_absolute`). Ruby's `Pathname` is an object that also
does filesystem IO (delegating to `File`/`Dir`/`FileUtils`). The string half is
partly done; the object + IO half is missing — but the IO primitives already
exist in `fs`/`io`, so this is mostly a wrapper.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Pathname.new(str)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Wrap a path string as an object. |
| `#+` / `#join(*p)` | [implemented] PARTIAL | `Path.join(a,b)` | Have 2-arg join; add variadic + object form. |
| `#parent` / `#dirname` | [implemented] PARTIAL | `Path.parent` | |
| `#basename([suffix])` / `#file_name` | [implemented] PARTIAL | `Path.file_name` | No suffix-strip arg. |
| `#extname` | [implemented] DONE | `Path.extname` | |
| `#sub_ext(ext)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Replace extension. |
| `#split` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | `[dirname, basename]`. |
| `#absolute?` / `#relative?` | [implemented] PARTIAL | `Path.is_absolute` | Add `relative?`. |
| `#cleanpath` / `#expand_path` / `#realpath` / `#realdirpath` | [implemented] PARTIAL | `fs.canonicalize` | `realpath` ≈ canonicalize; `cleanpath`/`expand_path` (no-IO) missing. |
| `#relative_path_from(base)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Compute a relative route. |
| `#each_filename` / `#ascend` / `#descend` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Component iteration. |
| `#root?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#exist?` / `#directory?` / `#file?` / `#symlink?` | [implemented] PARTIAL | `fs.exists`/`is_file`/`is_dir` | Wrap onto the object. |
| `#readable?` / `#writable?` / `#executable?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Permission probes. |
| `#size` / `#mtime` / `#atime` / `#ctime` / `#birthtime` | [implemented] PARTIAL | `fs.metadata` | Surface via metadata (times need §3 `Time`). |
| `#read` / `#write` / `#open` / `#each_line` / `#readlines` / `#binread` / `#binwrite` | [implemented] PARTIAL | `fs.read_to_string`/`write`, `io.File` | Wrap. |
| `#children` / `#entries` / `#glob` / `#opendir` | [implemented] PARTIAL | `fs.read_dir` | Have flat listing; `glob` (pattern match) missing. |
| `#mkdir` / `#mkpath` / `#rmdir` / `#rmtree` / `#unlink`/`#delete` | [implemented] PARTIAL | `fs.create_dir`/`create_dir_all`/`remove_dir_all`/`remove_file` | Primitives exist; wrap onto the object. |
| `#rename` / `#chmod` / `#chown` / `#utime` | [implemented] PARTIAL | `fs.rename` | `chmod`/`chown`/`utime` missing. |
| `#to_s` / `#to_path` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Unwrap. |

---

## 19. `fileutils` — PARTIAL · `library/std/fs` (primitives present, no FileUtils namespace)

The `fs` module already provides most primitives (`create_dir_all` = `mkdir_p`,
`remove_dir_all` = `rm_rf`, `copy` = single-file `cp`, `rename` = `mv`,
`symlink`, `canonicalize`). Missing: recursive copy (`cp_r`), `touch`, `mv`
across filesystems, `chmod`/`chown`, and the `FileUtils.*` naming.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `FileUtils.mkdir(dir)` | [implemented] PARTIAL | `fs.create_dir` | |
| `FileUtils.mkdir_p(dir)` / `makedirs` | [implemented] DONE (equiv) | `fs.create_dir_all` | |
| `FileUtils.rmdir(dir)` | [implemented] PARTIAL | — | Single empty-dir remove (fs has recursive only). |
| `FileUtils.rm(f)` / `rm_f` | [implemented] PARTIAL | `fs.remove_file` | `rm_f` (ignore-missing) not distinct. |
| `FileUtils.rm_r(dir)` / `rm_rf` | [implemented] DONE (equiv) | `fs.remove_dir_all` | |
| `FileUtils.cp(src, dst)` | [implemented] DONE (equiv) | `fs.copy` | Single file. |
| `FileUtils.cp_r(src, dst)` / `cp_lr` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Recursive directory copy. |
| `FileUtils.mv(src, dst)` | [implemented] PARTIAL | `fs.rename` | `rename` fails across filesystems; `mv` should fall back to copy+delete. |
| `FileUtils.ln(old, new)` (hard link) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `FileUtils.ln_s(old, new)` / `ln_sf` | [implemented] PARTIAL | `fs.symlink` | Have symlink; `ln_sf` (force) missing. |
| `FileUtils.touch(f)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Create/update mtime. |
| `FileUtils.chmod(mode, f)` / `chmod_R` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Permission bits. |
| `FileUtils.chown(u, g, f)` / `chown_R` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Ownership. |
| `FileUtils.install(src, dst, mode:)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Copy + set mode. |
| `FileUtils.compare_file(a, b)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Byte-compare. |
| `FileUtils.copy_stream(src, dst)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Stream copy (via `io`). |
| `FileUtils.pwd` / `cd(dir){}` | [implemented] PARTIAL | `env.current_dir` | Have pwd; `cd` (chdir) missing. |
| `FileUtils.uptodate?(new, old_list)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | mtime comparison (needs §3). |
| `Dir.glob(pattern)` / `Dir[pattern]` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Pattern-based listing (also §18 `#glob`); commonly needed. |
| `Dir.mktmpdir` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Ties to `tempfile` (§20). |
| `Find.find(*paths){\|p\| }` (the `find` stdlib) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Recursive walk (fs has flat `read_dir` only). |

---

## 20. `tempfile` — MISSING · no module

Temp files/dirs. Needed for large uploads, atomic writes, test fixtures. Builds
on `fs`/`io` + `securerandom` (unique names) + `env` (tmpdir).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Tempfile.new(basename[, tmpdir])` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Create + open a uniquely named temp file. |
| `Tempfile.create(basename){\|f\| }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Block form; auto-unlink on exit. |
| `#path` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | The temp path. |
| `#close(unlink_now = false)` / `#close!` / `#unlink` / `#delete` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Cleanup. |
| `#open` (reopen) / `#length` / `#rewind` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Reuse. |
| `Dir.mktmpdir([prefix]){\|dir\| }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Temp directory (see §19). |

---

## 21. `comparable` / `enumerable` — COMPILE-TIME (mixin protocols) · cross-ref `core.md`

Both are *mixins* realized in Ruby by including a module that provides default
methods over a single required method (`<=>` for Comparable, `each` for
Enumerable). In Ruxen these are trait-like mixins with default method bodies —
compile-time, not runtime. Full method lists live in [`core.md`](core.md); this
row-set records the protocol and the key surface a parity mixin must provide.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Comparable` via `<=>`: `< <= == > >= between?(a,b) clamp(a,b)` / `clamp(range)` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | Mixin over a required `<=>`. Set/Time/Date all need it. |
| `Enumerable` via `each`: `map/collect select/filter reject find/detect` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | Mixin over a required `each` (+ optional `<=>` for min/max/sort). |
| … `reduce/inject each_with_index each_with_object` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | |
| … `group_by partition chunk chunk_while slice_when tally` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | |
| … `sort sort_by min max min_by max_by minmax` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | Needs `<=>`. |
| … `sum count first take drop take_while drop_while` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | |
| … `flat_map zip each_slice each_cons cycle` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | |
| … `all? any? none? one? include? find_index` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | |
| … `to_a/entries to_h to_set uniq` | [implemented] COMPILE-TIME: the mixin exists; core.md has each method's state | core.md | |
| … `lazy` (Enumerator::Lazy) | [implemented] PARTIAL | `library/std/iter` | Ruxen has an iterator layer; map `lazy` onto it. |

---

## 22. `delegate` (`SimpleDelegator` / `DelegateClass`) — COMPILE-TIME · no module

Object-level delegation. `DelegateClass(Klass)` forwards `Klass`'s public
instance methods (a compile-time-known set → generate forwarders).
`SimpleDelegator` forwards *any* method via `method_missing` — the tier-2
substitute is "forward the statically known method set of the wrapped type."

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `DelegateClass(Klass)` → subclass with forwarders | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Generate a forwarder for each public method of `Klass`. |
| `SimpleDelegator.new(obj)` (forward everything) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Forward the wrapped type's known methods; no runtime `method_missing`. |
| `#__getobj__` / `#__setobj__` | [implemented] PARTIAL/COMPILE-TIME | — | `__getobj__` trivial; `__setobj__` requires same-typed replacement. |
| `Delegator` base / `#method_missing` catch-all | [retired] OUT-OF-SCOPE | — | Arbitrary runtime dispatch; substitute is the generated forwarder set. |

---

## 23. `observer` — MISSING · no module (buildable in pure Ruxen)

The Observable mixin is plain runtime state (a list of callbacks) — no
metaprogramming needed. Buildable directly as a mixin holding an `Array` of
observer closures.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `include Observable` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Mixin holding the observer list. |
| `#add_observer(obj, func = :update)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Register (closure/callback in Ruxen, not a symbol). |
| `#delete_observer(obj)` / `#delete_observers` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Unregister. |
| `#count_observers` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#changed(state = true)` / `#changed?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Dirty flag. |
| `#notify_observers(*args)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Fire callbacks when changed. |

---

## 24. `logger` — MISSING · no module

Structured logging. Straightforward over `io`/`stdout` + `sync` (thread-safe
writes) + `time` (§3 for timestamps). P1 — every service needs it.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Logger.new(logdev, shift_age = 0, shift_size = ...)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Sink = stdout/stderr/file; rotation optional. |
| Levels `DEBUG < INFO < WARN < ERROR < FATAL < UNKNOWN` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Level enum. |
| `#debug/#info/#warn/#error/#fatal/#unknown(progname){ msg }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Level methods (message or block). |
| `#add(sev, msg, progname)` / `#log` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Generic entry. |
| `#level` / `#level=` / `#debug?`/`#info?`/… | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Threshold. |
| `#formatter=` / `Logger::Formatter` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Custom line format (closure over sev/time/progname/msg). |
| `#datetime_format=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Timestamp format (needs `strftime` §3). |
| `#progname=` / `#<<(raw)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Metadata / raw write. |
| `#close` / `#reopen` / `#with_level(lvl){}` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Lifecycle. |
| `Logger::LogDevice` (rotation by age/size) | [implemented] PARTIAL | — | Rotation is the harder part; ship non-rotating first. |

---

## 25. `optparse` — MISSING · no module (buildable in pure Ruxen)

CLI option parsing — plain string processing, not metaprogramming, so
directly buildable. P2 (Ruxen apps are mostly servers, but tools need it).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `OptionParser.new { \|o\| … }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Builder. |
| `#on(*opts, desc) { \|val\| }` / `#on_head` / `#on_tail` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Declare a flag + handler (closure). |
| `#banner=` / `#separator(str)` / `#program_name=` / `#version=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Help/usage text. |
| `#parse!(argv)` / `#parse(argv)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Consume args (mutating vs not). |
| `#order` / `#order!` / `#permute` / `#permute!` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Ordering modes. |
| Typed coercion (`Integer`, `Float`, `Array`, custom via `#accept`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Convert option values. |
| Required/optional argument syntax (`--flag VAL` / `[VAL]`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Arg presence. |
| `#help` / `#to_s` / `#summarize` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Generated help. |
| `#getopts(short, *long)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | One-shot convenience. |
| `--version` / `--help` auto handling | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `OptionParser::ParseError` / `InvalidOption` / `MissingArgument` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Error types (Ruxen: `Result`). |
| Lighter alternatives `GetoptLong`, `Getopt` | [retired] OUT-OF-SCOPE | — | Legacy; OptionParser subsumes. |

---

## 26. `benchmark` — PARTIAL · `library/std/bench`

Ruxen has a `Bencher` (iteration harness + `black_box`), and `time.Instant`
gives wall-clock realtime. Missing: the Ruby `Benchmark.bm`/`bmbm` report format
and the `Tms` (utime/stime/cutime/cstime/real) CPU-time breakdown.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Benchmark.realtime { }` → Float seconds | [implemented] PARTIAL | `Instant.now`/`.elapsed` | Compose from the monotonic clock. |
| `Benchmark.measure(label) { }` → `Tms` | [implemented] PARTIAL | `bench.Bencher` | Have an iteration harness; not the `Tms` shape. |
| `Benchmark.bm(width) { \|x\| x.report(label){} }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Tabular report. |
| `Benchmark.bmbm(width) { }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Rehearsal + measure. |
| `Benchmark::Tms` (`utime stime cutime cstime real total`, `#+ #- #/ #* #format`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Needs CPU-time (getrusage); wall-clock only today. |
| `Bencher#iter` / `black_box` | [implemented] DONE | `bench.Bencher` | Ruxen-native (Criterion-style), no Ruby analog. |

---

## 27. `thread` / `mutex` / `queue` — PARTIAL · `library/std/sync`

`sync` provides real OS threads (`Thread.spawn`/`JoinHandle`), `Mutex[T]`,
atomics, and channels (`Sender`/`Receiver` ≈ `Queue`), plus `Signal` (condvar-
ish) and `SharedSync`. Gaps vs Ruby: ergonomic `Thread.new`/`#value`/`#name`,
`Thread.list`/`current`/`pass`, a first-class `Queue`/`SizedQueue`, and
`ConditionVariable` with the standard `wait/signal/broadcast`.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Thread.new { }` / `Thread.start` | [implemented] PARTIAL: `Thread.spawn` today; Ruby `Thread.new { }` [planned](../../TODO.md#t-thread-api) | `Thread.spawn(closure: Int)` | Exists but takes a raw fn-ptr Int, not an ergonomic typed closure. |
| `#join([limit])` / `#value` | [implemented] PARTIAL: `JoinHandle.join` today; `Thread#value` [planned](../../TODO.md#t-thread-api) | `JoinHandle.join` | Have join; typed `#value` return + timeout gap. |
| `#alive?` / `#status` / `#stop?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Thread state. |
| `Thread.current` / `Thread.main` / `Thread.list` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Registry (partial: `ThreadId` exists). |
| `Thread.pass` / `Thread.stop` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Scheduler hints. |
| `#name=` / `#name` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | (`ThreadId` present, not names.) |
| `#kill` / `#raise` / `#wakeup` / `#run` | [retired] OUT-OF-SCOPE | — | Async interruption of another thread; no safe equivalent (informs `timeout` §31). |
| `#[]` / `#[]=` (thread-local) / `Thread.current.thread_variable_*` | [planned](../../TODO.md#t-thread-api) MISSING | — | Thread-local storage. |
| `#abort_on_exception=` / `Thread.report_on_exception=` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Panic policy (Ruxen: `ThreadPanic`). |
| `Thread.sleep` / `sleep(sec)` | [implemented] DONE | `Thread.sleep(&Duration)` | |
| `Mutex#lock/#unlock/#try_lock/#synchronize/#locked?/#owned?` | [implemented] PARTIAL: `Mutex[T].lock` and `MutexGuard` today; `Mutex#synchronize` [planned](../../TODO.md#t-thread-api) | `Mutex[T].lock/lock!`, `MutexGuard` | RAII guard model; `synchronize`/`try_lock`/`locked?` gaps. Ruxen's mutex is typed-data-owning (Rust-style), Ruby's guards a critical section. |
| `Mutex#sleep(timeout)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Release-and-wait. |
| `Queue` (`#push/#<<`, `#pop/#shift/#deq`, `#size/#length`, `#empty?`, `#clear`, `#close`, `#closed?`, `#num_waiting`) | [implemented] PARTIAL: a channel today; `Thread::Queue` [planned](../../TODO.md#t-thread-api) | `Sender`/`Receiver` (channel) | Channel ≈ unbounded Queue; no unified `Queue` object / `close`/`num_waiting`. |
| `SizedQueue` (`#max`, bounded `#push`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Bounded/backpressure channel. |
| `ConditionVariable` (`#wait(mutex, timeout)`, `#signal`, `#broadcast`) | [implemented] PARTIAL | `Signal` | `Signal` is condvar-adjacent; map to the standard 3-method API. |
| `Thread::Backtrace` / `#backtrace` | [retired] OUT-OF-SCOPE | — | Runtime stack introspection. |

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
| Cooperative concurrency / scheduling | [implemented] DONE (substitute); the Fiber scheduler that later replaces `async`/`await` is [planned](../../TODO.md#t-fibers) | `future` (async/await, tasks, scheduler) | The intended Ruxen model. |
| `Fiber.yield(*args)` (cooperative yield point) | [planned](../../TODO.md#t-fibers) PARTIAL | `Task.yield_now` | Yield exists; value passing on resume does not. |
| `Fiber.new { }` / `#resume(*args)` | [planned](../../TODO.md#t-fibers) PARTIAL | `spawn`/`Task` | Task spawn ≈ fiber; no manual step-by-step `resume` with values. |
| `#alive?` / `Fiber.current` | [planned](../../TODO.md#t-fibers) PARTIAL | task handles | Task handle exists; no `Fiber.current` global. |
| `Fiber#transfer` (symmetric coroutines) | [retired] OUT-OF-SCOPE | — | Symmetric control transfer; no compiled equivalent. Use channels/tasks. |
| `Fiber.scheduler=` / `Fiber.set_scheduler` (non-blocking IO hook) | [planned](../../TODO.md#t-fibers) PARTIAL | `future` reactor + `async_net`/`async_io` | Ruxen's reactor is the scheduler; it isn't a pluggable Ruby-level object. |
| `Fiber#storage` / `Fiber[]` (fiber-local) | [planned](../../TODO.md#t-fibers) MISSING | — | Task-local storage. |
| `Enumerator` built on Fiber (external iteration) | [implemented] PARTIAL | `library/std/iter` | Ruxen iterators cover most external-iteration uses. |

---

## 29. `monitor` — MISSING · no module

A **reentrant** mutex + condition variables. Ruxen's `Mutex` is non-reentrant, so
this needs a distinct reentrant lock (owner + recount) plus condvars.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Monitor.new` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Reentrant lock. |
| `#synchronize { }` / `#mon_synchronize` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Reentrant critical section. |
| `#mon_enter` / `#mon_exit` / `#mon_try_enter` / `#mon_locked?` / `#mon_owned?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Manual lock ops. |
| `#new_cond` → `MonitorMixin::ConditionVariable` (`#wait`, `#wait_while`, `#wait_until`, `#signal`, `#broadcast`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Monitor-scoped condvar. |
| `include MonitorMixin` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Mix reentrant locking into a class. |

---

## 30. `timeout` — MISSING · no module

`Timeout.timeout(sec){ block }` that raises after `sec`. Ruby implements it with
a watcher thread that calls `Thread#raise` on the target — Ruxen has no async
thread interruption (§27), so arbitrary *blocking* code can't be force-cancelled.
Feasible cases: cooperative/async code (cancellation tokens) and syscalls that
take timeouts (socket read/write already do, §9/net).

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Timeout.timeout(sec, klass = nil, msg = nil) { }` (blocking code) | [implemented] PARTIAL (cooperative code) / [retired] OUT-OF-SCOPE (interrupting arbitrary blocking code) | — | No `Thread#raise`; can't interrupt arbitrary blocking work. |
| Timeout for async tasks | [implemented] PARTIAL | `future` (`race` / `select` with a sleep) | Race a `Task.sleep` against the work; cooperative cancel. |
| Timeout for socket IO | [implemented] DONE (equiv) | `net.TcpStream.set_read/write_timeout` | Per-syscall timeouts already exist. |
| `Timeout::Error` / `Timeout::ExitException` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Ruxen: a `Result`/error variant on the async path. |

---

## 31. `prime` / Integer number theory — MISSING · no module

The `prime` stdlib plus the number-theory Integer methods. (Core `Integer` methods
like `gcd`/`lcm`/`pow(b,m)`/`Integer.sqrt`/`digits`/`bit_length` are tracked in
[`core.md`](core.md); listed here for the app-facing "number theory" cluster.)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Prime.prime?(n)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Primality test. |
| `Prime.each(ubound){ \|p\| }` / `Prime.take(n)` / `Prime.first(n)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Prime generation (sieve/lazy). |
| `Prime.prime_division(n)` / `Prime.int_from_prime_division(pd)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Factorization ↔ reconstruction. |
| `Integer#prime?` / `Integer.each_prime(ub)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Integer-side sugar. |
| `Integer#gcd` / `#lcm` / `#gcdlcm` | [planned](../../TODO.md#t-parity-stdlib) MISSING | core.md | Number theory (also core). |
| `Integer#pow(b, mod)` (modular exp) | [planned](../../TODO.md#t-parity-stdlib) MISSING | core.md | Needed for crypto. |
| `Integer.sqrt(n)` (isqrt) | [planned](../../TODO.md#t-parity-stdlib) MISSING | core.md | |
| `Integer#digits(base)` / `#bit_length` / `#to_s(base)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | core.md | Base conversion / bit ops. |

---

## 32. `matrix` — MISSING · no module (low priority)

Linear algebra. Rare in web/API apps; useful for graphics/ML. Low priority.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Matrix[[…],[…]]` / `Matrix.rows` / `Matrix.columns` / `Matrix.build(r,c){}` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Construction. |
| `Matrix.identity(n)` / `.zero` / `.diagonal` / `.scalar` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Special matrices. |
| `#+ #- #*` (matrix/scalar) / `#**` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Arithmetic. |
| `#determinant/#det` / `#inverse/#inv` / `#transpose/#t` / `#rank` / `#trace/#tr` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Operations. |
| `#[]` / `#row(i)` / `#column(j)` / `#row_count` / `#column_count` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Access. |
| `#eigen` / `#eigensystem` / LUP decomposition | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Advanced. |
| `Vector[…]` (`#+ #- #* #inner_product/#dot #cross_product #magnitude/#norm #normalize`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Companion Vector type. |

---

## 33. `bigdecimal` — MISSING · no module

Arbitrary-precision decimal — required for money/financial math (float is
wrong there). (`Rational`/`Complex` are core numeric types — see [`core.md`](core.md).)

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `BigDecimal(str[, precision])` / `"1.5".to_d` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Construct from string (exact). |
| `#+ #- #* #/` / `#div(x, n)` / `#% / #modulo` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Exact arithmetic with scale. |
| `#power(n[, prec])` / `#** / #sqrt(prec)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `#round(n[, mode])` / `#truncate` / `#ceil` / `#floor` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Rounding modes (banker's etc.). |
| `#to_s(fmt)` / `#to_f` / `#to_i` / `#to_r` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Conversions. |
| `#precision` / `#exponent` / `#sign` / `#nan?` / `#infinite?` / `#finite?` / `#zero?` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Introspection. |
| `BigMath` (`exp`, `log`, `PI`, `E`, `sin`, `cos`, `atan`, `sqrt`) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Transcendental at precision. |

---

## 34. Marshal / `pp` / misc utility libs

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Marshal.dump(obj)` / `Marshal.load(str)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | Binary object serialization → per-type generated ser/de (serde-style). Arbitrary-graph runtime marshal is out of scope; typed codegen is the substitute. |
| `PP.pp(obj)` / `Kernel#pp` / `#pretty_inspect` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Pretty-printer over `inspect` (core.md); buildable. |
| `PrettyPrint` (the engine) | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Wadler/Lindig algorithm; underlies `pp`. Low priority. |
| `Shellwords.escape(str)` / `.split(line)` / `String#shellsplit` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Shell-safe quoting for `process`/`Command` (§Open3). |
| `Abbrev.abbrev(words)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Unique-prefix table; low value. |
| `English` (`$INPUT_RECORD_SEPARATOR` aliases) | [retired] OUT-OF-SCOPE | — | Global-variable aliases; not applicable. |
| `SecureRandom` (see §4) / `Digest` (§5) / `Base64` (§6) | see those sections: [implemented] PARTIAL, [planned](../../TODO.md#t-parity-stdlib) MISSING, [implemented] DONE | — | Cross-refs. |

---

## 35. `open3` — PARTIAL · `library/std/process`

`process` provides `Command` with `.arg/.args/.env/.current_dir/.status/.output`
— `Command.output` already gives captured stdout+stderr+status (≈ `capture3`).
Missing: the streaming `popen3` (concurrent read/write of the child's pipes) and
pipelines.

| Ruby API | Status | Ruxen module or gap | Note |
|----------|--------|---------------------|------|
| `Open3.capture3(*cmd, stdin_data:, binmode:)` → `[out, err, status]` | [implemented] PARTIAL | `Command.output` → `Output{stdout, stderr, status}` | Have capture; no `stdin_data` feed. |
| `Open3.capture2(*cmd)` / `capture2e(*cmd)` | [implemented] PARTIAL | `Command.output` | Subset of the above. |
| `Open3.popen3(*cmd){ \|i,o,e,t\| }` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Streaming pipes (concurrent IO with the child). |
| `Open3.popen2(*cmd)` / `popen2e(*cmd)` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | |
| `Open3.pipeline(*cmds)` / `pipeline_r` / `pipeline_w` / `pipeline_rw` / `pipeline_start` | [planned](../../TODO.md#t-parity-stdlib) MISSING | — | Chained processes. |
| `Command.new(prog).arg/.args/.env/.current_dir` | [implemented] DONE | `process.Command` | Builder present. |
| `Command#status` / `#output` | [implemented] DONE | `process.Command` | `ExitStatus{code, success}` + `Output`. |
| `Process.spawn` / `Process.wait` / `Process.kill` / `system` / backtick | [implemented] PARTIAL | `Command.status`/`.output`, `Process.exit` | High-level run present; low-level `spawn`/`wait`/signal control missing. |

---

## Out of scope (no runtime; tier-2 substitute noted)

These need a live metaobject runtime / GC / string-eval that Ruxen deliberately
omits. Each row names the compile-time substitute.

| Ruby API | Status | Substitute |
|----------|--------|-----------|
| `eval` / `instance_eval(str)` / `class_eval(str)` / `binding` | [retired] OUT-OF-SCOPE | Compile-time macros/codegen; templates compiled at build time (§13). |
| `ObjectSpace.each_object` / `count_objects` / `_id2ref` | [retired] OUT-OF-SCOPE | No heap walk; use explicit registries. |
| `WeakRef.new(obj)` / `WeakMap` | [retired] OUT-OF-SCOPE | No GC/weak refs; use ownership + explicit lifetimes, or an id→value map with manual eviction. |
| `GC.start` / `GC.stat` / `GC.disable` / `ObjectSpace::WeakMap` | [retired] OUT-OF-SCOPE | No GC (ownership-based); nothing to tune. |
| `TracePoint.new(:call){}` | [retired] OUT-OF-SCOPE | Compile-time instrumentation / explicit hooks. |
| `DRb` (distributed Ruby) | [retired] OUT-OF-SCOPE | Use `net`/`rondo` RPC over a real protocol. |
| `coverage` / `Ripper` / `RubyVM::*` | [retired] OUT-OF-SCOPE | Compiler-internal; not a user library. |
| `irb` / `reline` / `readline` (interactive REPL library) | [retired] OUT-OF-SCOPE | Ruxen ships a `repl` binary; not a linkable stdlib. |
| `did_you_mean` | [retired] OUT-OF-SCOPE | Compile-time diagnostic (compiler already suggests). |
| `rbconfig` (`RbConfig::CONFIG`) | [implemented] PARTIAL | `env` covers runtime host info (`cpu_count`, `vars`, `current_dir`). |
| `set` `SortedSet`, `matrix` eigen, `bigdecimal` `BigMath` | [planned](../../TODO.md#t-parity-stdlib) MISSING (tracked in the rows above; low priority) | Tracked above; low priority. |

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
12. **`json` option gaps + generated `to_json` (§1)** — object↔JSON mapping
    (compile-time serializer) so structs round-trip without hand-built nodes.

**P2 — ergonomics + completeness:**
13. **`set` completion (§2)** — subset/superset/`^`/merge + Enumerable mixin.
14. **`Struct`/`Data` codegen (§14)** + **`forwardable` (§15)** + **`singleton`
    (§16)** — the compile-time-synthesis batch (shared macro/codegen machinery,
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
