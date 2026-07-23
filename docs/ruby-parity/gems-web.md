# Web / API parity — Rails (Action Pack) + Sinatra on `rondo`

Maps the Sinatra + Rails web-layer surface onto **rondo** (`~/.projects/rondo`),
the Ruxen web framework, and gives the **compile-time** Ruxen design for each
feature. Ruby's web stack leans on runtime metaprogramming (controllers via
`send`-dispatch, `params` magic, route-DSL macros, ERB `eval`, Rack middleware
built at boot). Ruxen is a pure compiler: no interpreter, no `send`, no `eval`,
no runtime class mutation. Every ergonomic below is delivered by **compile-time
machinery** instead, per the tiers in [`README.md`](README.md).

Cross-references: [`gems-db.md`](gems-db.md) for the ORM model structs the
serializers/params/pagination designs build on, and [`stdlib.md`](stdlib.md)
for `digest` / `hmac` / `base64` / `openssl` / `uri` / `net-http` / `net-smtp`,
which most of the security surface (sessions, CSRF, JWT, mailers, WebSocket
handshake) is gated on.

## How to read this

Every area is a table:

| Rails / Sinatra feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|

`rondo status`: **HAS** (in `src/*.rx` today), **PARTIAL** (a piece exists),
**MISSING**. `Needs` names the concrete unit of work: a `rondo .rx` change, a
`compiler synthesis` (a new mixin + auto-derive-walker target, the Ruxen
equivalent of a Rust `derive`), a `toolchain` build step (a new `ruxen`
subcommand / preprocessor), or a `stdlib` dependency (cross-ref
[`stdlib.md`](stdlib.md)).

## The compile-time toolbox (read once, referenced throughout)

Ruxen has no user-facing macros and no runtime reflection, so "metaprogramming"
resolves to one of five concrete levers. Every design below is built from these.

1. **Blocks + builder DSL (works today).** rondo already registers routes with a
   trailing block: `app.get(&"/x") { |req: &Request| body(&req) }`. This is
   Ruby-block semantics (`docs/decisions/ruby-block-semantics.md`). It is the
   Sinatra ergonomic verbatim. Constraint: closure bodies are single-expression
   (rondo **W4**), so real work lives in free fns the closure calls.

2. **Mixins + the auto-derive walker (the ActiveRecord/serializer lever).** A
   `mixin` (`include Serializable`) can carry synthesized methods computed from a
   struct's fields, the same mechanism that derives `Send`/`Sync` FFI widths
   today (`library/std/hash` B2 walker). This is where `to_json`, `from_params`,
   validations, and finder-style methods land: the compiler generates the method
   bodies over the struct's declared fields at compile time. New derive targets
   (`Serialize`, `Deserialize`, `FromParams`) are **compiler synthesis** work,
   but they reuse an existing, proven mechanism rather than inventing one.

3. **Runtime registration loops (the route-table lever).** Anything Rails does
   with a load-time DSL macro (`resources :users` expanding to 7 routes) can be a
   **plain Ruxen method that calls the existing `app.get/post/...` in a loop at
   startup**. No new compiler machinery: it is ordinary code that runs once in
   `main` before `listen()`. This covers `resources`, `namespace`, `scope`,
   `constraints`, and `mount`.

4. **Toolchain codegen / preprocessor (the ERB + route-helper lever).** For
   surfaces that genuinely need generated *source* (typed `users_path` helpers,
   ERB-style templates compiled to render fns), a `ruxen` build step reads the
   DSL/template file and emits `.rx` before compilation. This is how views reach
   Rails ergonomics without runtime eval: the template compiles to a Ruxen
   function whose escaping is guaranteed at compile time. New toolchain work, but
   it keeps the "pure compiler, fully-formed program" invariant.

5. **Typed structs replacing hashes (the strong-params / session lever).** Where
   Rails passes an untyped `params` / `session` hash, Ruxen prefers a typed
   struct deserialized once at the boundary. `permit`/`require` become the
   struct's field set; a missing required field is a parse error, not a runtime
   `nil`. This is lever 2 (a `FromParams` derive) applied to the request boundary.

**What is genuinely out of scope (tier 3):** `send(action_name)` with an action
computed from an arbitrary runtime string, `method_missing` finders on unknown
names, `ActiveSupport`'s open-class monkey-patching, and `instance_eval` view
contexts. Each has a compile-time substitute below (a fixed dispatch `match`, a
generated method set, a mixin, a builder), because every *practical* web use of
them ranges over a **fixed, compile-time-known set**.

## The stdlib gate (why half of "Rails" is blocked, and on what)

rondo's framework surface is close to Sinatra-complete. The Rails surface that is
missing is missing because of **stdlib crypto/net gaps**, not framework design.
Verified against `library/std/` (module list) on 2026-07-23:

| Capability | Stdlib today | Blocks these web features |
|---|---|---|
| SHA-256 / SHA-1 / MD5 digest | **MISSING** (`std/hash` is the HashMap runtime, not crypto) | signed sessions, CSRF tokens, JWT, WebSocket `Sec-WebSocket-Accept`, ETags |
| HMAC | **MISSING** | signed cookies, CSRF, JWT (HS256) |
| base64 / base64url | **MISSING** | JWT, cookie payloads, Basic auth, data URIs |
| AES / OpenSSL | **MISSING** | *encrypted* (not just signed) sessions, `Rails.application.credentials` |
| CSPRNG (`SecureRandom`) | **PARTIAL** — `std/rand` exists; confirm it is OS-seeded/CSPRNG before using for tokens | session ids, CSRF tokens, API keys |
| `net-http` client | **PARTIAL** — `std/http` is GET-only, sync, plain HTTP (no POST, headers, TLS) | upstream API calls, OAuth, webhooks |
| TLS / HTTPS | **MISSING** (`std/net` is TCP-only) | HTTPS serving, SMTPS/STARTTLS, WSS, HTTPS client |
| `net-smtp` | **MISSING** | ActionMailer delivery |
| `uri` | **MISSING** as a module (rondo has inline `url_decode`/query parsing in `helpers.rx`) | `URI.parse`, `url_for` host/query building, redirect-safety checks |

Read this table as the critical path: **land `digest` + `hmac` + `base64` in
stdlib and most of the Rails security surface unlocks at once.**

---

## 1. Routing

rondo's router is `src/route.rx` (`Route.matches`) + `src/rondo.rx`
(`dispatch`, the `get/post/...` registrars) + `src/router.rx` (late-binding
helpers). Matching is pre-split literal segments with `:name` captures and
optional inline `:id(\d+)` regex constraints.

| Rails / Sinatra feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| `get/post/put/patch/delete` | **HAS** | `Rondo.get/post/put/patch/delete(path, handler)` (`rondo.rx:399-417`). Sinatra verbatim. | — |
| `head` / `options` | **HAS** | Explicit `app.head` / `app.options` (`rondo.rx:424-434`); HEAD else falls back to the GET route (`find_get_route_for`, `router.rx:44`). | — |
| Named route params `:id` | **HAS** | `:name` segment → capture, read via `req.param(&"id")` (`request.rx:110`); merged over query params in `dispatch` (`merge_params_into`). | — |
| Regex constraints `:id(\d+)` | **HAS** | Inline anchored regex per segment (`Route.param_constraints`, `segment_satisfies`, `helpers.rx:418`) over `std/regex`. Rails `constraints: { id: /\d+/ }` equivalent. | — |
| Query params `?a=b` | **HAS** | Parsed into `req.params` (`parse_query_into`, `helpers.rx:138`), URL-decoded (`%xx`+`+`). `req.param` reads route-or-query transparently (Sinatra). | — |
| Splat / wildcard `/files/*path` | **MISSING** | Add a `*name` segment kind to `Route`: matches one-or-more trailing segments, captured as a single `/`-joined string. Extend `Route.new` seg classification + `matches` (variable-arity tail). | `rondo .rx` (`route.rx`) |
| Full-pattern regex routes | **MISSING** | A `Route` variant holding a compiled `Regex` over the whole path with named groups → `req.params`. Register via `app.match(&regex, handler)`. | `rondo .rx` + `std/regex` named-group capture |
| `resources :users` (7 REST routes) | **MISSING** | Lever 3: a `Rondo.resources(name, controller_bindings)` method that, at startup, pushes the 7 `Route`s (index/show/new/create/edit/update/destroy) by calling the existing registrars in a loop. No macro: it is a normal method run before `listen()`. | `rondo .rx` |
| `resource :profile` (singular) | **MISSING** | Same method, the 6-route singular variant (no `:id`, no index). | `rondo .rx` |
| Nested resources | **MISSING** | `resources` takes an optional parent-prefix arg; nesting composes prefixes at registration time. | `rondo .rx` |
| Route helpers `users_path`, `user_path(u)` | **MISSING** | Two designs. (a) v1: a typed `Routes` struct with `def users_path -> String` etc., hand-written or emitted. (b) Rails-parity: a `toolchain` step reads the `resources` DSL and **generates** `*_path`/`*_url` free fns as `.rx`, so helpers are compile-time-checked and refactor-safe. | `rondo .rx` (a) → `toolchain` codegen (b) |
| `namespace :admin` / `scope "/v1"` | **MISSING** | Lever 3: `Rondo.namespace(prefix) { ... }` prepends `prefix` to every route registered inside the block. Pure runtime prefix concatenation. | `rondo .rx` |
| `constraints(subdomain:, format:)` | **PARTIAL** | Host/subdomain constraints need the `Host` header on `Request` (present in `headers`); add a constraint predicate checked in `dispatch` before handler invocation, mirroring the existing per-segment regex gate. | `rondo .rx` |
| `mount RackApp` | **MISSING** | Ruxen has no Rack, but a sub-app mount = register a catch-all prefix route delegating to another `Rondo` instance's `dispatch`. `app.mount(&"/api", sub_app)`. | `rondo .rx` |
| `redirect` route / `root to:` | **PARTIAL** | `root` = `app.get(&"/", handler)`. Inline redirect routes = a handler returning `Response.redirect` (`response.rx:145`). | — |
| Verb override (`_method` form field) | **MISSING** | A built-in before-hook: when method is POST and `req.form` has `_method=PATCH|DELETE`, rewrite `req.method`. Rails/Rack `MethodOverride`. | `rondo .rx` |

---

## 2. Controllers

Rails dispatches `UsersController#show` by `send`-ing the action name resolved
from the route at runtime. Ruxen has no `send`. The compile-time model: a
controller is a **struct**, each action is a **method**, and the route binds a
**specific method reference** as its handler. The `resources` helper (area 1)
generates these bindings, so the wiring is written once.

| Rails feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| Controller class + action methods | **MISSING** (rondo handlers are closures/free fns) | `class UsersController` with `def show(req: &Request) -> Response`. Route binds the method: `app.get(&"/users/:id") { \|r\| UsersController.new(r).show }` (W4-thin closure). `resources` emits these bindings. Dispatch is a direct call, not a name lookup. | `rondo .rx` (a `Controller` base mixin) |
| Action selected by `params[:action]` | **OUT-OF-SCOPE (tier 3) → fixed dispatch** | The route already knows its action at registration, so no runtime string→method dispatch is needed. Where an app truly needs "action name from a fixed set", it is a `match action_name; "show" -> ...; "edit" -> ...` over the known set, not `send`. | — |
| Merged `params` (route+query+body) | **PARTIAL** | Route+query already merged into `req.params` (`dispatch`). Body params via `req.form` (urlencoded + multipart, `request.rx:137`). Provide `req.params_all` merging `form` over `params` so a controller reads one map, Sinatra-style. | `rondo .rx` |
| Strong params `require`/`permit` | **MISSING** | Lever 5: a per-action **typed params struct**. `struct UserParams { name: String; email: String; bio: Option[String] }` with `include FromParams` synthesizing `UserParams.from(req) -> Result[UserParams, ParamError]`. `permit` = the field set; `require(:user)` = a non-`Option` nested struct; unpermitted keys are dropped by construction. Compile-time-checked shape, no `permit!` escape hatch to misuse (global rule 4). Coercion/validation cross-refs `gems-db.md`. | `compiler synthesis` (`FromParams` derive) |
| `before_action` / `after_action` | **PARTIAL** | rondo has **global** `before`/`after` hooks (`rondo.rx:116-126`). Per-controller filters = methods a `Controller` mixin calls around the action: the generated action wrapper runs `before_action` methods (each returning `Option[Response]` to halt), the action, then `after_action`. `only:`/`except:` are compile-time lists on the controller. | `rondo .rx` (`Controller` mixin) |
| `around_action` | **MISSING** | An around filter is a method taking the action as a `&block`: `def with_timing(req, &block) -> Response`. Ruby-block semantics support this today (`&block: Fn[(...) -> Response]`, yielding the action). | `rondo .rx` |
| `skip_before_action` | **MISSING** | Compile-time: the filter list is per-controller data; "skip" omits the entry for that action's `only:`/`except:` set. No runtime removal. | `rondo .rx` |
| `rescue_from` | **PARTIAL** | rondo has `on_error(status, handler)` (status-keyed, `rondo.rx:133`). Exception-class-keyed rescue = wrap the action call in `match action(); Ok(r) -> r; Err(e) -> handler(e)` where actions return `Result`. Pairs with typed error enums. | `rondo .rx` (`Controller` mixin) |
| `render json:/text:/html:` | **HAS** | `Response.json/text/html` (`response.rx:23-45`). `render json: model` composes with the serializer (area 10). | — |
| `render template:` / implicit view | **MISSING** | Render a compiled view fn (area 8): `Response.html(&UsersShow.render(&user))`. Implicit "render the action's view" = the generated action wrapper calls the conventionally-named view fn. | `toolchain` (view compiler) |
| `render status:` / `head :no_content` | **HAS** | `.with_status(code)` (`response.rx:72`); `head` = `Response.text(&"").with_status(204)`. | — |
| `redirect_to` | **HAS** | `Response.redirect(&url)` / `redirect_with_status` (`response.rx:145-160`). `redirect_back` needs the `Referer` header (present in `req.headers`). | — |
| `respond_to` / content negotiation | **MISSING** | Parse the `Accept` header (+ `.json`/`.html` path suffix) into a format enum; the controller `match`es on it to pick `Response.json` vs `.html`. A `req.format -> Format` helper + a `respond_to` builder that maps `Format -> Response`. | `rondo .rx` (needs `uri`/media-type parse, or inline) |
| `params.to_unsafe_h` / mass assign | **OUT-OF-SCOPE** | Deliberately absent: mass assignment from an untyped hash is the anti-pattern strong-params fixes. Typed structs only. | — |

---

## 3. Request / Response surface

`Request` = `src/request.rx`; `Response` = `src/response.rx`.

| Feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| Request headers (case-insensitive) | **HAS** | `req.header(&"x")` (`request.rx:105`); lowercased on parse (`parse_header_into`). | — |
| Response headers | **HAS** | `resp.with_header(&"x", &"y")` (`response.rx:77`), lowercased on insert. | — |
| Read cookies | **HAS** | `req.cookie(&"name")` / `req.cookies` (`request.rx:35`), URL-decoded (`parse_cookies_into`). | — |
| Set cookies (+ secure) | **HAS** | `with_cookie` / `with_secure_cookie` (HttpOnly;Secure;SameSite=Lax) / `with_cookie_attrs` (`response.rx:101-142`). Multi-cookie safe via `set-cookie:<name>` storage keys. | — |
| **Sessions** (signed cookie) | **MISSING** | A `Session` = typed struct serialized to JSON, HMAC-signed, base64'd, written via `with_secure_cookie`. A `before` hook verifies the signature and deserializes into `req.session`; an `after` hook re-signs if mutated. Rails' default `cookie_store`. | `stdlib`: `hmac`+`digest`+`base64`; then `rondo .rx` |
| **Sessions** (encrypted) | **MISSING** | Same, but AES-GCM-encrypt the payload before base64 (Rails 5+ default). | `stdlib`: `openssl` (AES-GCM); then `rondo .rx` |
| Server-side session store | **MISSING** | Session id (CSPRNG) in the cookie; payload in a store (in-process `Map` for dev; Postgres/Redis via `gems-db.md`). `Rondo` is `include Sync`+`Send`, so a shared store is sound. | `stdlib`: CSPRNG; store via `gems-db.md` |
| **Flash** | **MISSING** | A one-shot sub-map of the session: a `before` hook loads `flash`, an `after` hook clears the keys that were read. Depends on sessions. | sessions (above) |
| **CSRF protection** | **MISSING** | Per-session HMAC token; a `before` hook rejects unsafe verbs (POST/PUT/PATCH/DELETE) whose `authenticity_token` field or `X-CSRF-Token` header does not match; a `csrf_token` view helper emits the hidden field. | `stdlib`: `hmac`+CSPRNG; then `rondo .rx` |
| Content-Type set / detect | **HAS** | Constructors set it; `req.form` dispatches on it (`request.rx:139`). | — |
| `send_file` (download from disk) | **PARTIAL** | Static serving exists (`try_static_or_404` + `serve_open_file` + `mime_type_for`, `router.rx:72`/`helpers.rx:123`). `send_file` = that plus a `Content-Disposition: attachment; filename=` header. Add `Response.file(&path, disposition)`. | `rondo .rx` |
| `send_data` (download from memory) | **PARTIAL** | `Response.with_body` + explicit `content-type` + `Content-Disposition`. Add a `Response.attachment(&bytes, &filename, &ctype)` builder. | `rondo .rx` |
| **Streaming responses** (chunked) | **MISSING** | rondo buffers the whole response (`to_http` builds one `String`, tracked open in `remaining-tasks.md`). Design: `Response.stream { \|out\| ... }` writing chunks to the socket with `Transfer-Encoding: chunked`; needs the async writer surface + wire-layer chunk framing. | `rondo .rx` (async writer) + reactor |
| `request.ip` / `remote_addr` | **PARTIAL** | Thread the accepted peer address from `async_server.rx` onto `Request`; honor `X-Forwarded-For` behind a trusted-proxy flag. | `rondo .rx` |
| `request.xhr?` / body-size guard | **PARTIAL** | `xhr?` = check `X-Requested-With`. Size guards HAS: `max_header_size`/`max_body_size` → 431/413 (`rondo.rx:234-243`, `request_size_status`). | — |
| ETag / conditional GET | **MISSING** | Weak/strong ETag = hash of the body; a `before`/`after` weave comparing `If-None-Match` → 304. | `stdlib`: `digest`; then `rondo .rx` |

---

## 4. Views / templates

No runtime `eval`, so ERB/Haml/Slim cannot be interpreted. Two compile-time
paths, offered in order of increasing Rails-fidelity.

| Feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| HTML rendering | **PARTIAL** | `Response.html(&str)` HAS (`response.rx:40`). The v1 path is a **Ruxen builder DSL**: `tag.div(class: "card") { tag.span(user.name) }` producing an escaped `String`, escaping by construction (global rule 13, no `raw`). Same shape as quiver's builder DSL. Works today via blocks. | `rondo .rx` (an `Html` builder module) |
| ERB / Slim templates | **MISSING** | The Rails-ergonomic path: a `toolchain` **template compiler**. `ruxen build` preprocesses `app/views/**/*.rhtml` into render fns: `<%= expr %>` → an **escaped** append, `<%== %>` → raw append (opt-in), `<% stmt %>` → control flow, yielding `def render(<typed locals>) -> String`. Escaping is guaranteed at compile time; a typo in a local is a **compile error**, not a runtime `NoMethodError`. | `toolchain` (new `.rhtml`→`.rx` compiler) |
| Layouts | **MISSING** | A layout is a render fn taking the inner content as a `&block`/String: `Layout.render(title) { inner }`. `yield` in ERB → the block. Composes in both the builder and template-compiler paths. | as above |
| Partials `render "form"` | **MISSING** | A partial is just another compiled render fn called by name; `locals:` are its typed parameters. Collection partials = a loop calling it. | as above |
| View helpers (`link_to`, `form_with`) | **MISSING** | Helpers are free fns returning escaped `String`/builder nodes: `link_to(text, path)`, `form_with(model, url) { fields }`. `form_with` emits the CSRF hidden field (area 3). | `rondo .rx` (a `helpers` module) |
| HTML escaping | **DESIGN** | Guaranteed by construction: the builder/template only ever appends via an escaping writer; there is no `html_safe`/`raw` to bypass it (global rules 13/27). Raw output is an explicit, greppable `<%== %>` / `Html.raw` opt-in. | `rondo .rx` |
| `content_for` / `provide` | **MISSING** | Named content slots = a `Hash[String, String]` threaded through the layout render call; set in the view, read in the layout. | as above |
| I18n (`t(...)`) | **MISSING** | Locale files compiled to a typed lookup fn (`toolchain` step or a generated `match`); missing keys are compile errors. | `toolchain` or `rondo .rx` |
| Asset helpers (`asset_path`) | **MISSING** | A build-time asset manifest → a generated `asset_path(name) -> String` returning the fingerprinted URL. Ties into `gems-frontend.md`. | `toolchain`; cross-ref `gems-frontend.md` |

---

## 5. Middleware (Rack)

Ruxen has no Rack. The equivalent is rondo's **hook chain** (`before` /
`after` / `on_error`), woven uniformly through `dispatch` and finalized in
`finish` (`rondo.rx:391`). A "middleware" is a before-hook (returns
`Some(response)` to short-circuit) plus optionally an after-hook (rewrites the
response).

| Rack middleware | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| The stack itself | **HAS (as hooks)** | Ordered `before_hooks`/`after_hooks` arrays; before-`Some` short-circuits (`dispatch` step 3), after rewrites (step 6). Registration order = execution order. | — |
| `Rack::Logger` | **HAS** | `app.logging` → one logfmt line per request at `finish` (`format_log_line`, `rondo.rx:378`). | — |
| `Rack::Static` | **HAS** | `set_public_folder` + `try_static_or_404`, `..`-traversal-rejected, MIME by extension. | — |
| `Rack::Cors` | **HAS** | `cors` + `cors_*` setters; OPTIONS preflight → 204; `Vary: Origin` (`cors_response`, `rondo.rx:324`). | — |
| Request-ID (`ActionDispatch::RequestId`) | **HAS** | `app.request_id` propagates inbound `X-Request-ID` or mints `req-<n>`; echoed on every path (`request_id_for`/`with_request_id`). | — |
| `Rack::Session` | **MISSING** | A before/after hook pair over signed/encrypted cookies (area 3). | sessions stdlib gate |
| `Rack::Protection` (CSRF, host) | **PARTIAL** | Host allow-list = a before-hook on the `Host` header (works today). CSRF = area 3 (blocked on crypto). | `rondo .rx` + crypto for CSRF |
| `Rack::Deflater` (gzip) | **MISSING** | An after-hook gzipping the body when `Accept-Encoding: gzip`. Needs a deflate implementation in stdlib. | `stdlib`: `zlib`/deflate |
| `Rack::Attack` (throttle) | **MISSING** | A before-hook with a per-key counter store (in-process `Map`+`AtomicUsize`; durable via `gems-db.md`). See area 9 rate limiting. | `rondo .rx` (+ store) |
| `Rack::MethodOverride` | **MISSING** | The `_method` before-hook (area 1). | `rondo .rx` |
| Custom middleware | **HAS** | Write a before/after fn; `app.before(fn)` / `app.after(fn)`. Same power as a Rack middleware for the request/response cycle. | — |

---

## 6. Serialization

Rails: `to_json`, `ActiveModel::Serializer`, jbuilder, all runtime reflection
over attributes. Ruxen: **lever 2**, a `Serializable` mixin whose auto-derive
walker synthesizes `to_json` over the struct's declared fields at compile time.
`std/json` (`Json.object`/`stringify`, present) is the value backend. This is the
single highest-leverage item and it composes directly with the `gems-db.md`
model structs.

| Feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| `model.to_json` | **PARTIAL** (`std/json` builds values manually) | `include Serializable` on a struct → synthesized `to_json -> Json` iterating declared fields into `Json.object` (`std/json`). The Ruxen `derive(Serialize)`. | `compiler synthesis` (`Serialize` derive) over `std/json` |
| `JSON.parse` → struct | **PARTIAL** | `include Deserializable` → synthesized `from_json(&Json) -> Result[Self, JsonError]`, field-typed, missing/typed-wrong = `Err`. Powers request bodies + upstream API responses. | `compiler synthesis` (`Deserialize` derive) |
| `only:` / `except:` / `methods:` | **MISSING** | Attribute selection = a serializer *view struct* (a projection struct with only the exposed fields) or a generated `to_json_only(fields)`. Explicit projection over runtime key-filtering. | `compiler synthesis` |
| Custom serializer class | **MISSING** | A dedicated `UserSerializer` struct with a hand-written `to_json` when the derive is not enough (computed fields, renames via a field attribute). | `rondo .rx` (per-model) |
| Nested / `has_many` embedding | **MISSING** | The derive recurses into fields that are themselves `Serializable`; association arrays serialize element-wise. Pairs with `gems-db.md` `includes` to avoid N+1 before serializing. | `compiler synthesis`; cross-ref `gems-db.md` |
| jbuilder-style ad-hoc JSON | **HAS** | Build `Json.object`/`Json.array` directly (`std/json`), or the builder DSL shape for JSON. | — |
| XML serialization | **MISSING** | Rare; a `to_xml` derive analogous to `to_json`, or the `tag.*` builder. Deferred. | `compiler synthesis` (low priority) |

---

## 7. Mailers (ActionMailer)

ActionMailer = a template (subject/body) + SMTP delivery. The template half
reuses the view compiler (area 4). The delivery half is **blocked on stdlib**:
there is no `net-smtp`, and `std/net` has no TLS, so no SMTPS/STARTTLS.

| Feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| Mailer class + methods | **MISSING** | `class UserMailer` with `def welcome(user) -> Email`; each method builds a typed `Email { to, subject, html_body, text_body }`. | `rondo .rx` (or a `mailer` package) |
| Email templates (html+text) | **MISSING** | Compiled render fns (area 4), one per part. Multipart body assembled from both. | `toolchain` (view compiler) |
| Layouts / helpers in mail | **MISSING** | Same as web views (area 4). | as above |
| SMTP delivery | **MISSING** | An `Smtp` client over `std/net` `TcpStream` speaking the SMTP dialog. Plain SMTP works over TCP; real providers need STARTTLS/SMTPS. | `stdlib`: **`net-smtp`** + **TLS**; base64 for AUTH LOGIN |
| Deliver later (async) | **MISSING** | `deliver_later` = enqueue a mail job (area 8). | jobs (area 8) |
| Attachments | **MISSING** | base64-encoded MIME parts. | `stdlib`: `base64` |
| Preview / interceptors | **MISSING** | Preview = render the template to a dev route (works once templates land). Interceptors = a delivery hook. | `toolchain` + `rondo .rx` |

---

## 8. Background jobs (ActiveJob / Sidekiq)

rondo already runs a **thread-per-core async runtime** (`async_server.rx`,
`Task.spawn_raw`). A job system is an enqueue API + a backing store + workers
draining it. In-process is pure Ruxen today; durable needs a store (`gems-db.md`)
and interacts with the open reactor-waker work in `remaining-tasks.md`.

| Feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| Job class + `perform` | **MISSING** | `class WelcomeJob` with `include Job` / `def perform(user_id: Int)`. Dispatch is a direct method call on a typed job struct, not a string class-name lookup. | `rondo .rx` (a `Job` mixin) |
| `perform_later` (enqueue) | **MISSING** | `WelcomeJob.new(id).enqueue(queue)` serializes args (via the `Serialize` derive, area 6) into a store entry. | `compiler synthesis` (Serialize) + store |
| In-process worker pool | **PARTIAL** | Drain the queue with `Task.spawn` on the existing reactor / a dedicated worker thread. `Rondo` is `Send`+`Sync`. | `rondo .rx`; reactor-waker work (`remaining-tasks.md`) |
| Durable queue (Redis/PG) | **MISSING** | The store is a table/list via `gems-db.md`; enqueue = insert, worker = `SELECT ... FOR UPDATE SKIP LOCKED` drain loop. | cross-ref `gems-db.md` |
| Retries / backoff | **MISSING** | Runtime: an attempt counter on the job row, re-enqueue with a delay on `Err`. | store + timer |
| Scheduled / `set(wait:)` | **MISSING** | A `run_at` column; the worker polls due jobs. Needs the timer future path. | store + reactor timers |
| Unique / dedup, priorities | **MISSING** | Store-level: a uniqueness key column, a priority-ordered drain. | store |
| Callbacks (`after_perform`) | **MISSING** | Methods the `Job` mixin wraps around `perform` (mirror of controller filters, area 2). | `rondo .rx` |

---

## 9. API mode

| Feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| JSON request/response | **HAS / PARTIAL** | `Response.json` HAS; request-body parse + model serialize via the `Deserialize`/`Serialize` derives (area 6). | `compiler synthesis` (derives) |
| API versioning | **MISSING** | Route `namespace`/`scope` prefixing (`/v1`, area 1); or an `Accept` header/version-param before-hook selecting the handler set. | `rondo .rx` |
| Pagination | **MISSING** | A query object over the ORM (`limit`/`offset` or keyset) from `gems-db.md`; emit `Link`/`X-Total-Count` headers. Params via strong-params. | cross-ref `gems-db.md` |
| Rate limiting | **MISSING** | A before-hook keyed on ip/token → a counter store (in-process `Map`+`AtomicUsize`, or durable via `gems-db.md`); over-limit → 429 with `Retry-After`. | `rondo .rx` (+ store) |
| **CORS** | **HAS** | `cors` + `cors_*` (area 5). | — |
| Token auth (opaque) | **PARTIAL (buildable today)** | A before-hook reading `Authorization: Bearer <t>`, looking the token up via `gems-db.md`, attaching the user or returning 401. No crypto needed. | `rondo .rx`; cross-ref `gems-db.md` |
| **JWT** (HS256) | **MISSING** | `header.payload.signature`, base64url each part, HMAC-SHA256 the first two, constant-time compare. Verify in a before-hook. | `stdlib`: `hmac`+`digest`+`base64url` |
| JWT (RS256) | **MISSING** | RSA sign/verify. | `stdlib`: `openssl` |
| Basic auth | **MISSING** | Decode `Authorization: Basic <b64>`, split `user:pass`, verify. | `stdlib`: `base64` |
| API keys / HMAC request signing | **MISSING** | HMAC over the request; constant-time compare in a before-hook. | `stdlib`: `hmac` |
| Error envelope / problem+json | **PARTIAL** | A typed `ApiError` struct serialized via the derive; an `on_error`/`rescue_from` weave maps it to the right status. | `compiler synthesis` + `rondo .rx` |

---

## 10. WebSocket / ActionCable / SSE / streaming

| Feature | rondo status | Compile-time Ruxen design | Needs |
|---|---|---|---|
| **WebSocket** protocol | **MISSING** (rondo TODO) | HTTP `Upgrade` handshake: `Sec-WebSocket-Accept = base64(SHA1(key + GUID))`, then RFC 6455 frame parse/encode on the kept-alive socket via the reactor. Protocol-level, dedicated effort. | `stdlib`: `digest`(SHA1)+`base64`; `rondo .rx` (framing) + reactor |
| ActionCable (pub/sub channels) | **MISSING** | On top of WebSocket: a `Channel` struct with `subscribed`/`receive`; a broadcast registry (a `Sync` `Map[channel, Array[conn]]`) fanning messages to subscribers. | WebSocket (above) + shared registry |
| **SSE** (`text/event-stream`) | **MISSING** | Simpler than WS: a streaming `Response` (area 3) that writes `data:` frames and flushes; no handshake. Good first streaming target. | `rondo .rx` (async writer) + reactor |
| Live/`ActionController::Live` | **MISSING** | Same streaming-response primitive. | streaming (above) |
| Turbo Streams (over WS/SSE) | **MISSING** | Once WS/SSE + the view compiler land, broadcast rendered HTML fragments. Ties into `gems-frontend.md`. | WS/SSE + view compiler; cross-ref `gems-frontend.md` |

---

## Build order (the implementation sequence)

Ordered so each stage ships a coherent capability and unblocks the next. Stages
1 and 2 need no new compiler or stdlib work.

**Stage 0 — Sinatra parity is basically here (verify + close small gaps).**
Confirm the framework surface in `remaining-tasks.md` (routing, hooks, CORS,
cookies, static, multipart, size limits). Close the cheap `rondo .rx` routing
gaps: splat/wildcard segments, `resources`/`namespace`/`scope` as
startup-registration methods (lever 3), the `_method` override hook, `send_file`/
`send_data` (`Content-Disposition`), `request.ip`, `req.params_all`. Outcome:
Sinatra-complete + REST route tables, all pure Ruxen.

**Stage 1 — Controllers + strong params + filters.** Land the `Controller`
mixin (per-controller `before_action`/`after_action`/`around_action`,
`rescue_from`), and the `FromParams` derive for typed strong params (the first
`compiler synthesis` item, but small and high-value). Wire `resources` to emit
controller-method bindings. Outcome: a Rails-shaped controller layer with
compile-time-checked params and thin, W4-safe handlers.

**Stage 2 — Serialization + JSON API.** Land the `Serialize`/`Deserialize`
derives over `std/json` (lever 2). This immediately gives model→JSON, JSON→struct
request bodies, nested embedding, and the error envelope. Add API versioning
(route prefixes) and pagination (query object from `gems-db.md`). Add opaque
**token auth** (no crypto). Outcome: a full JSON API stack minus signed tokens.

**Stage 3 — Views + templates.** Ship the `Html` builder DSL first (pure
`rondo .rx`, escaping by construction), then the `toolchain` `.rhtml`→`.rx`
template compiler for Rails-ergonomic ERB (layouts, partials, typed locals,
compile-time escaping) and view helpers (`link_to`, `form_with`). Outcome:
server-rendered HTML with no runtime eval.

**Stage 4 — The crypto stdlib gate (unblocks the security surface).** Land
`std/digest` (SHA-256/SHA-1), `std/hmac`, `std/base64`, and confirm/`std/rand`
CSPRNG. This is the critical path from [`stdlib.md`](stdlib.md). It unblocks, in
one stroke: **signed cookie sessions**, **flash**, **CSRF**, **JWT (HS256)**,
**Basic auth**, **ETags**, and the **WebSocket handshake**. Then wire the
`Session`/`flash`/`csrf` hook weave in `rondo .rx`.

**Stage 5 — Jobs.** In-process job queue on the existing reactor (`Job` mixin,
`enqueue` via the Serialize derive, worker drain), then durable queues over
`gems-db.md`. Coordinate with the reactor-waker work in `remaining-tasks.md`.
Unblocks `deliver_later`.

**Stage 6 — Streaming + realtime.** Add the streaming `Response` primitive
(chunked writer over the async socket) → **SSE** first, then the **WebSocket**
frame layer (handshake from stage 4) and an ActionCable-style channel registry.
Turbo Streams follow, tying into [`gems-frontend.md`](gems-frontend.md).

**Stage 7 — Mailers + TLS.** Land `std/net` **TLS** and a `net-smtp` client,
then ActionMailer (templates from stage 3, delivery over SMTPS/STARTTLS,
attachments via base64, `deliver_later` via stage 5). TLS also unlocks HTTPS
serving, an HTTPS client, and WSS.

**Deferred:** encrypted (vs signed) sessions and RS256 JWT (need `std/openssl`
AES/RSA), gzip (`Rack::Deflater`, needs deflate), HTTP/2, and route-helper source
generation (`toolchain`), each added when a concrete app demands it.

---

### Provenance

rondo surface verified against `~/.projects/rondo/src/{rondo,request,response,route,router,helpers}.rx`
and `docs/remaining-tasks.md` (2026-07-23). Compile-time levers verified against
`ruxen/docs/decisions/ruby-block-semantics.md`, the retired-`@[...]`-attribute
note (`ruby-naming.spec.md §10a`), and the `std/hash` auto-derive walker.
stdlib gaps verified against the `library/std/` module list (`json`, `http`,
`net`, `regex`, `time`, `rand` present; no `digest`/`hmac`/`base64`/`openssl`/
`uri`/`net-smtp`). Cross-refs [`gems-db.md`](gems-db.md),
[`stdlib.md`](stdlib.md), [`gems-frontend.md`](gems-frontend.md) are being
authored in parallel; reconcile status rows when they land.
