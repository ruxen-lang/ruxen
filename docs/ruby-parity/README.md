# Ruby parity — the master tracker

Goal: **Ruxen reaches full parity with Ruby (CRuby / MRI) syntax, language
features, core + stdlib, and the major gem ecosystems (DB, web/API, frontend),
so a Ruby developer can build a full app in Ruxen without missing features.**

This directory is the authoritative, no-skip backlog. Every Ruby feature is
listed here with a status, so nothing is silently dropped as "v1". We grind it
in large batches, not one-off chunks.

## The design truth (read first)

Ruby is dynamically typed with a live metaobject runtime; Ruxen is compiled,
statically typed, ownership-based, no GC. **Ruxen stays a pure compiler: no
interpreter, no runtime class mutation, no `eval` of runtime strings.** The
strategy is to do at COMPILE TIME what Ruby does at runtime, so the emitted
program is fully formed and never mutates itself. Three tiers:

1. **Direct parity** — features expressible as-is (syntax, blocks, most core
   methods, control flow, modules/mixins, exceptions). Implement fully.
2. **Compile-time parity (the main lever)** — everything Ruby does with load-time
   OR runtime metaprogramming is re-expressed with Ruxen's compile-time
   machinery: `derive`, mixins, and (where needed, new) macros / codegen /
   attribute-driven synthesis. `attr_accessor`, `define_method`,
   `method_missing`-style dynamic dispatch, ActiveRecord attribute/finder
   methods, Rails routing DSLs, serializers, validations, delegation
   (`Forwardable`), `Struct`/`Data` — all become code the compiler GENERATES,
   not methods installed at runtime. The ergonomics match; the mechanism is
   synthesis at compile time. This tier is where most "metaprogramming" lands,
   and it is the priority: prefer a compile-time design over calling anything
   out of scope.
3. **No compile-time expression (rare, tracked with why)** — only the truly
   irreducible: `eval` of a string not known until runtime, `binding`,
   `TracePoint`, live production monkey-patching of already-compiled code. We do
   NOT add a runtime for these. For each, record the closest compile-time design
   that covers the real use case (almost every practical use of these has one:
   e.g. "dynamic method name from a fixed set" is a macro/match, not `send`).

## The matrices (audited, one file per area)

Each file is a table: Ruby feature/method -> `DONE | PARTIAL | MISSING | COMPILE-TIME | OUT-OF-SCOPE`,
with the Ruxen location or the gap. Filled by the audit pass, then burned down.

- `language.md` — syntax + semantics: literals, blocks/procs/lambdas, splat +
  keyword args, ranges, symbols, `case`/pattern matching, exceptions, modules,
  visibility, operator overloading, `method_missing`/`define_method` (tier 2/3),
  refinements, `self`/`super`, comparable/enumerable protocols, string
  interpolation/heredocs/format, iterators.
- `core.md` — core classes: `String`, `Symbol`, `Integer`, `Float`, `Rational`,
  `Complex`, `Array`, `Hash`, `Range`, `Set`, `Struct`, `Comparable`,
  `Enumerable`, `Kernel`, `Object`, `NilClass`/`TrueClass`/`FalseClass`,
  `Proc`, `Method`, `Regexp`, `Time`.
- `stdlib.md` — stdlib: `json`, `set`, `date`/`time`, `securerandom`, `digest`,
  `base64`, `uri`, `net/http`, `openssl`, `csv`, `logger`, `optparse`, `fileutils`,
  `pathname`, `stringio`, `benchmark`, `ostruct`, `forwardable`, `singleton`.
- `gems-db.md` — DB parity: ActiveRecord + Sequel API surface mapped onto a
  compile-time Ruxen ORM over `ledger` (models, migrations, associations,
  validations, query builder, scopes, callbacks, connection pooling,
  transactions). Postgres/MySQL adapters beyond SQLite.
- `gems-web.md` — web/API parity: Rails + Sinatra surface mapped onto rondo
  (routing DSL, controllers, params, sessions, middleware, content negotiation,
  serializers, template/view layer, CSRF, cookies, mailers, jobs, WebSocket).
- `gems-frontend.md` — the frontend framework surface (components, state, forms,
  routing, data binding) mapped onto quiver, plus asset/build story.

## Process (how we do not skip)

1. Audit fills each matrix from the Ruby docs, exhaustively.
2. Sort each matrix by tier + value; batch the DONE-able items.
3. Implement in large batches (a whole class's methods, a whole DSL), each with
   tests + a `tests/release-e2e/` pin, updating the matrix row in the same change.
4. `PARTIAL`/`MISSING` never silently becomes `DONE` without a test.
5. Tier-3 items get a written rationale + their tier-2 substitute.

## Status

- **2026-07-23** — Tracker created; the design tiers set. Audit pass launched to
  fill the six matrices (language, core, stdlib, gems-db, gems-web, gems-frontend).
  Implementation batches start as each matrix lands.
