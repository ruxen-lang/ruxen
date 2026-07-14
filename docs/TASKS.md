# Ruxen — Task Index

**Canonical task list for the Ruxen compiler/toolchain.** This is a roll-up
over the authoritative backlogs below — it does not replace them, it tells you
where each kind of work lives and what is open *right now*. Keep it current
(see `../CLAUDE.md` → "Task tracking & keeping context current").

## Where work is tracked

| Source | What it holds |
|--------|---------------|
| [`requirements/ROADMAP.md`](requirements/ROADMAP.md) | The full tier-1→5 feature backlog (38 spec docs) **and** the P0 pre-flight bug list. The authoritative long-horizon plan. |
| [`dev/gui-stack-v1-issues.md`](dev/gui-stack-v1-issues.md) | Compiler/toolchain bugs surfaced by the sibling apps (Q1–Q22), each with a repro + severity + fix status. The app↔language ledger. |
| [`prompts/v1/25_v1_release_checklist.md`](prompts/v1/25_v1_release_checklist.md) | The v1.0.0 release gate checklist. |
| [`../CHANGELOG.md`](../CHANGELOG.md) | What's **done** — every landed fix (Keep a Changelog, `[Unreleased]`). |
| `tests/release-e2e/cases/` | The pin test backing each landed fix. |

## Open now — GUI-stack ledger (`dev/gui-stack-v1-issues.md`)

37 of 40 fixed (Q23–Q26 surfaced 2026-06-08; Q16 fixed 2026-06-08 on
feat/drop-elaboration; Q29 audited 2026-06-09 — already sound, pinned; Q28, Q30,
Q31 fixed 2026-06-09; Q32, Q33 fixed 2026-06-10; Q17 fixed for generic free fns 2026-06-10; **Q34 fixed 2026-06-10 via the syntax-parity harness; Q37 fixed 2026-06-10 (root cause: a yielding/`&block` method poisoned an unrelated same-named free fn via a bare-name yield map — this covers BOTH the S1 name-collision symptom AND the S2 "generic `frame[S: PaintSurface]` gets a bogus `__block` in binary-consumes-library builds" symptom; they were the same bug) — quiver examples build again; Q38 fixed 2026-06-10 (bare-literal `String` local leaked); Q39 fixed 2026-06-11 (bare `""` in a tuple-element position expecting `String` stayed `&str` — rondo W21; fixed via `coerce_tuple_literal_elements`, pin `924`); Q35 fixed 2026-07-14 (struct/enum in-body includes now populate typeck's nominal mixin registry; pin `659`); Q36 fixed 2026-07-14 (parenthesized multi-argument `yield(a, b)` parsed as one tuple argument; pin `929`); Q46 fixed 2026-07-14 (paren-less no-arg `.new` now installs the runtime-mixin class-info header; pin `460`); Q47 fixed 2026-07-14 (an existential `any Mixin` call result now self-satisfies its exact carried bound instead of probing a nonexistent concrete method table; pin `473`); Q48 fixed 2026-07-14 (LLVM's lazy call resolver now maps residual mangled FFI method callees to their declared C symbols, matching Cranelift)**). The canvas
`Int`→`Float32` event-coord revert (unblocked by Q28/Q31) has LANDED
(canvas 143 green, sub-pixel pinned, live windowed loop verified).

### Toolchain / tests

- [x] **Implicit `main` — Ruby-style top-level execution (DONE 2026-07-14,
      `feat/implicit-main`).** After parse and before resolve, executable
      `TopLevelItem::Expr` nodes are moved in source order into a synthesized
      zero-argument, `nil`-returning `main` when no explicit entry point exists.
      Declarations remain top-level and forward-visible. An explicit `main`
      remains an AST no-op; combining it with loose statements retains E0728.
      Release-e2e pins `933`–`935`; focused resolver, syntax-parity, Q23, CLI
      smoke, and `ruxen test` checks cover the shared compile/build seam.
- [x] **Q48 — LLVM FFI-alias resolution (FIXED 2026-07-14,
      `fix/q48-llvm-ffi`).** LLVM Pass 0 already declared aliased FFI methods
      under their C symbols, but its lazy call resolver ignored
      `MirProgram::ffi_libs`; synthesized MIR calls such as `Formatter_new`
      therefore created fresh undefined mangled externs. LLVM now resolves the
      Ruxen-side and lib-qualified keys to the existing C declaration, matching
      Cranelift's import map. The LLVM IR pin rejects `String_*`/`Formatter_*`
      leaks and the String/interpolation backend fixtures link and run.
- [x] **WASM target (tier 4.03) — `ruxen compile --target wasm32-unknown-unknown`
      (DONE 2026-06-12, `feat/drop-elaboration`).** Spec
      `requirements/tier4_03_wasm_target.md` (path-fixed + checklist annotated),
      ADR `decisions/phase4-no-std-wasm.md`. LLVM backend → `wasm-ld` → `.wasm`
      (no libc/C runtime — reactor module). Top-level `def`s export by source
      name via the LLVM `export_name` attribute. No stdlib bootstrap (no_std
      reality → sidesteps the LLVM vtable-globals gap). **Headline bar passes:**
      `def add` → `.wasm` runs in node, `add(2,3)===5` asserted
      (`examples/05-wasm/`, `scripts/wasm_verify.sh`, pin
      `tests/wasm_codegen.rs`). LLVM lane brought up
      (`LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 --features llvm`); default
      build unchanged (Cranelift). **Staged remainder** (ADR): wasm32-wasi,
      bundled allocator (String/Array exports), `wasm_import`/host imports,
      `std.wasm`, the `wasm_export "custom"` rename directive, browser harness.
- [x] **no_std / embedded (tier 4.04) — non-slip bar DONE (2026-06-12,
      `feat/drop-elaboration`).** Spec `requirements/tier4_04_no_std_embedded.md`
      (path-fixed + checklist annotated), ADR `decisions/phase4-no-std-wasm.md`.
      `ruxen compile --no-std`: skips stdlib bootstrap, links without the Ruxen
      C runtime / `[system_libs]` (zero `ruxen_*` symbols), suppresses
      `ruxen_env_init` + primitive `*_fmt` synthesis. **E1400** rejects heap
      allocation in a no_std unit (`src/no_std.rs`, `docs/errors/E1400.md`,
      `tests/no_std_e1400.rs`). Bars: `examples/06-no-std/`,
      `scripts/no_std_verify.sh` (exit 42 + E1400). "core" = existing
      `library/std/core` package (decision #1). **Staged remainder:** the
      `no_std`/`panic_handler`/`global_allocator`/`no_mangle` source directives,
      the `core`/`std`/`alloc` re-export surface, the manifest `[package]
      no-std` key, the strict `-nostdlib` zero-libc Linux binary, thumbv7em, and
      `panic = "unwind"`.
- [x] **Cross-compilation (tier 4.02) — `--target <triple>` (DONE 2026-06-12,
      `feat/drop-elaboration`).** Spec `requirements/tier4_02_cross_compilation.md`,
      ADR `decisions/cross-compilation-linker-matrix.md`, guide
      `CROSS_COMPILE.md`. `ruxen compile/build/run/check --target`; host path
      byte-identical. Cranelift `isa::lookup` (`all-native-arch`), LLVM
      `TargetTriple::create`; linker matrix incl. two-stage Docker for Linux.
      Per-target `target/<triple>/<profile>/` + per-target runtime (no cache
      poisoning, pinned). `ruxen target list/add/remove` (add/remove = loud
      Err). **Both bars pass:** aarch64-linux in a `linux/arm64` container,
      x86_64-darwin under Rosetta (`scripts/cross_verify.sh`). Deferred to later
      tiers (recorded in §10 of the spec): wasm/no_std (4.03/4.04), the
      cfg-expr evaluator + `[target.<triple>.dependencies]` (4.01), the
      prebuilt-runtime HTTP fetch + CI matrix (4.06). Android = config-ready,
      NDK-gated (untested). Tests: `tests/cross_compile_triples.rs`,
      ruxenc `cache_integration::target_cache_isolation`.
- [x] **Syntax-parity harness — one syntax across compiler/fmt/repl/lsp/ide
      (DONE 2026-06-10, `feat/drop-elaboration`).** ADR
      `docs/decisions/syntax-parity-harness.md`. Two axes: per-surface
      conformance over the cases + stdlib + canvas/quiver/rondo corpus (491
      files), and structural pins (a compile-time exhaustiveness guard over
      `TopLevelItem`/`MixinItem`/`ImplItem`; an explicit, currently-empty
      accepted-but-compile-rejected divergence allowlist). The
      fmt axis (reparse-identity + idempotence) caught **Q34** (dropped
      grouping parens) plus four more fmt-destructiveness bugs — zero-arg
      `MethodCall`→field-access, method-visibility-section drop, `async`
      modifier drop — all fixed. Pins: `tests/syntax_parity.rs`,
      `src/ruxen_ide/tests/syntax_parity_ide.rs`,
      `tests/q34_fmt_grouping_parens.rs`, and a new `parity` phase in
      `tests/release-e2e/run.sh` (in `PHASES=all`).
- [x] **Q37 · S1 — yield/`&block`-method name-collides with a same-named free
      fn (FIXED 2026-06-10, `feat/drop-elaboration`).** A yielding/block-taking
      METHOD registered its bare name in the name-keyed `yield_fns` map, so an
      unrelated same-named generic free fn inherited a phantom `__block` →
      `could not infer type for parameter __block`. This broke every quiver
      example binary once canvas gained its `frame` methods (colliding with
      quiver's `frame` free fn in the flat-merged build). Fixed by deciding the
      synthetic `__block` LOCALLY from each function's own body
      (`resolve/funcs.rs`); the buggy name map + populator deleted. Pin:
      release-e2e 920. Verified: quiver counter example builds again. Ledger
      §Q37.

### Concurrency adoption (`decisions/zio-comparison-concurrency-adoption.md`, plan `dev/concurrency-impl-plan.md`)

- [x] **§4.1 First-class cancellation + leak-free `def drop` across `.await`
      (Feature #1 — COMPLETE, uncommitted on `feat/concurrency-adoption`)** —
      the foundation (spec §6 build order #1). Decision: NARROW path (Drop vtable
      slot inside the future's existing class_info; NO global `mixin Drop
      dispatch runtime`; no non-future layout change). All three increments
      landed + pinned (`391`–`398`). What a user can now rely on: a cancelled or
      drained async future runs its `def drop` (and its sub-futures' drops),
      deterministically and leak-free, via synchronous RAII. Increments:
      - [x] **Increment 1 — a drained future runs its `def drop`** (in progress on
            `feat/concurrency-adoption`, uncommitted). `Drop_dynamic_drop` routed
            through the future class_info; `ruxen_executor_drain_remaining` fires
            it. Pin `tests/release-e2e/cases/391_async_drop_on_drain.rx` + layout
            pins in `tests/mixin_vtables.rs`. Explicit-drain surface only; the
            automatic block_on-exit/Scope drain is deferred to §4.2.
      - [x] **Increment 2 — state-indexed drop glue** on the synthesised state
            machine (uncommitted on `feat/concurrency-adoption`). MIR
            `synthesize_future_state_machine_drop` drops each owned `__sub_i`
            sub-future via `Drop_dynamic_drop` + `ruxen_dealloc`, reverse decl
            order, each NULL-guarded (`if self.__sub_i != 0` — loop machines build
            `__sub_i` lazily, so a loop future drained before first pump has a zero
            field that must be skipped, else null-deref). Double-drop sound:
            `<Future>_drop` is the SOLE dropper (poll only borrows; pump never drops
            completed futures; spawned future tainted out of scope drop). Bare-named
            await-binding fields NOT dropped (uninitialised until their await
            resolves — partial-init UB). Pins `392`–`395`. Two FOLLOW-UPS surfaced:
            (a) dropping cross-suspend USER-DROP class locals needs eager-vs-late
            metadata threaded from async-lowering; (b) loop re-init overwrites a
            completed prior iteration's `__sub_i` without dropping the old one
            (pre-existing intra-loop LEAK in the loop builder, not a UAF — Codex
            LOW, relayed to user).
      - [x] **Increment 3 — cooperative `cx.cancelled?` bit** through `Context`.
            `_Thread_local` flag in executor.c (the user body never sees `cx`, so
            thread-scoped like the reactor handle) + `cx.cancelled?` /
            `ruxen_request_cancel`/`clear_cancel`/`cancel_requested`. A leaf future
            checks it at its await boundary and early-returns → Increment-2 drop
            glue fires. Cooperative (no preemption of await-free loops). ALSO
            fixed the pre-existing block_on-root-future leak it surfaced: drop
            elaboration now force-roots a never-consumed `__block_on_fut_*` Future
            local (the spec §4.1 block_on-exit drop). Pins `396`/`397`/`398`.
- [x] **§4.2 Structured `Scope` (keystone — Feature #2 COMPLETE, uncommitted on
      `feat/concurrency-adoption`).** `library/std/future/src/scope.rx` over the
      thread-local scheduler + §4.1 drop machinery + one C primitive
      (`ruxen_executor_cancel_task`, reentrancy-safe). What a user can rely on: a
      `Scope.open do |s| ... end` block owns its children — `s.spawn(fut)` (handle
      walled off), `s.collect` (sync pump, reactor-parking, fail-fast) /
      `s.collect_supervised`, and falling off the block cancels every still-pending
      child (fires its `def drop`). Single-reactor v1; Int error channel. Pins
      `399`–`403`. Surface choices (current-lowering): spawn takes a future VALUE
      (async closures unlowered), collect is SYNC (async methods unlowered) — both
      documented in scope.rx + the plan. FOLLOW-UPS surfaced: (a) `s.spawn(async_fn())`
      where the arg is an async-def WRAPPER call fails to link (`_<fn>` undefined) —
      spawn a direct stdlib future or a hand-written future for now; (b) typed
      `Result[T,E]` child channel awaits typed `TaskJoinFuture.Output`.
- [x] **§4.3 General `Race` (Feature #3 COMPLETE, uncommitted on
      `feat/concurrency-adoption`).** `library/std/future/src/race.rx` +
      `ruxen_race_*` C step in scheduler.c. `Race.of(a, b)` 2-arm +
      `Race.any_of(arr)` N-way (renamed from `any` — reserved keyword). First arm
      to complete wins; losers cancelled (their `def drop` fires via the §4.1
      `Drop_dynamic_drop`) before the winner is returned; winner block leaked
      (not dropped → heap payload stays valid). Results `Race2.First/Second(Int)`
      / `RaceN.Won(idx, Int)` (v1 Int channel). No new reactor primitive.
      Reentrancy-safe; empty `any_of` panics. Pins `417`–`422`. FOLLOW-UPS: (a)
      raw-`Int` arm move-contract not enforceable (shared spawn/scope/race gap —
      needs typed move-only future params); (b) typed-generic Output (the Int
      channel); (c) heap-payload result ergonomics — `p as String` on the Int
      payload prints the raw pointer, not the value (no typed re-hydration).
- [x] **§4.4 Async backpressured `Queue` (Feature #4 COMPLETE, uncommitted on
      `feat/concurrency-adoption`).** `library/std/future/src/queue.rx` + a C
      control block + step machines in `library/std/future/runtime/queue.c`.
      `Queue.bounded(capacity, overflow)` → a `QueueEnds` handle (pull
      `sender`/`receiver`; not the spec's `let (tx, rx)` tuple — class-tuple
      destructuring gap); capacity mandatory, no unbounded. `AsyncSender`/
      `AsyncReceiver` split; `send`/`recv` are hand-written futures that PARK THE
      TASK via the reactor (never block the OS thread), resolving
      `Result[Int, QueueError]`. `enum Overflow { Suspend; DropOldest; DropNewest }`
      (no-payload enum → int32 tag at heap offset 0, `Shutdown` ABI; unknown/null
      panics). Close→Err: last-sender-drop → recv `Err(QueueClosed)`; receiver-drop
      → parked Suspend-send `Err`. `share` = multi-producer. v1: SINGLE-REACTOR
      (cross-core wake deferred to v2, §4.4 risk 3 / §6), single in-flight parked
      waiter per direction, Int channel. Pins `423`–`428`. Codex fixes: cancel-
      while-parked waker UAF, `QueueEnds` duplicate-end UAF, capacity guard,
      overflow-tag-from-pointer ABI, unknown-tag panic. PARKED LOW (orchestrator):
      add an `Overflow` tag-stability pin (mirroring `shutdown_tag_stability.rs`).
      FOLLOW-UPS (v2/cross-cutting): cross-core wake + atomic refcounts; a waiter
      list for multi-producer backpressure + multi-consumer recv; spawned-multi-
      await self-reschedule; drop-rooting for stdlib-class method-result locals
      (→ automatic ownership-driven close); class-typed tuple destructuring
      (restore `let (tx, rx) = ...`); typed `Queue[T]` element channel.
- [x] **§4.5 Async `Stream` (Feature #5 COMPLETE, uncommitted on
      `feat/concurrency-adoption`).** `library/std/future/src/stream.rx` + a tiny
      `runtime/stream.c`. `mixin Stream dispatch runtime` over ONE required
      `def var poll_next(cx) -> Poll[Option[Int]]`, with `for_each`/`count`/`sum`
      default bodies (the async `Enumerable[T]` — and the first `dispatch runtime`
      mixin to also carry default-body methods). `for_each` is a SYNC pump driver
      (parks via `Scope.pump_or_park`, NOT a bare yield — pumps task-backed
      sources). Adapters `MapStream`/`FilterStream`/`TakeStream` own their inner
      as `any Stream` (dyn-mixin), poll via the runtime vtable, compose into
      chains, and are FUSED (latch on inner-`nil`). LEAK-FREE CHAINS (Inc4):
      dropping a chain drops every stage — the §4.1 Drop-vtable gate was widened
      to `Future OR Stream` so a `Stream` class gets a Drop vtable slot (uniform
      slot 1; layout-pinned), and each adapter's `def drop` runs its inner's
      `def drop` via `Drop_dynamic_drop` (pin 436 proves exactly-once via a
      live-count guard). v1: Int channel, single-consumer pull, leak-free
      drop-on-cancel + leak-free chains. Pins `429`–`436`. Codex fixes: `for_each`
      pump-on-Pending (pin 430), explicit-`cx.drop` removed, adapter fuse (pin
      435), Inc4 ABI/layout VALIDATED (no double-drop, slot-uniform). REPORTED
      sharp edge (pre-existing, NOT fixed): a class including BOTH `Future` AND
      `Stream` (incoherent) trips the slot-uniformity assert (loud panic) — a
      proper diagnostic is a maintainer call. FOLLOW-UPS (v2): fluent
      `.map`/`.filter`/`.take` (by-move-self); `for x in stream` (method-level
      async lowering); typed `Stream[T]`; concurrent stages; the for_each-then-
      drop drop-rooting fix; the `Future+Stream` incoherent-include diagnostic;
      Q46 (no-arg-init source crashing inside an adapter — a separate
      constructor-lowering bug, filed).
- [x] **§4.7 deterministic virtual `TestClock` (Feature #7 COMPLETE, uncommitted
      on `feat/concurrency-adoption`).** `library/std/future/runtime/test_clock.c`
      + edits to `time.c`/`reactor.c` + `library/std/future/src/test_clock.rx`.
      A `--test`-style mode: `TestClock.install`/`advance(d)`/`advance_to_next`/
      `set(d)`/`uninstall` swap the per-thread reactor's real OS timer for a
      virtual clock, so `Async.sleep`/timeouts resolve INSTANTLY when advanced
      past (pin 437: 60s sleep fires in 0.85s). Covers BOTH the async-sleep seam
      AND sync `Instant` (spec §4.7 Correction 1 — they sample the clock
      independently). HYBRID: real fd readiness untouched. **NO compiler change**
      (only the bootstrap-embedded sibling registration). v1: single-thread;
      explicit `uninstall` (the guard's `def drop` is the RAII intent but a
      method-result local isn't drop-elaborated — the Scope/Queue/Stream
      drop-rooting limit). Pins `437`–`443`. Codex fixes: a deregister blocker
      (fired virtual timer would `close` an fd / double-decrement registered_count
      — pin 442) + nested-install medium (depth-counted — pin 443). FOLLOW-UPS:
      the guard auto-restore (drop-rooting fix); a `static inline` clock seam
      (LTO closes the cross-TU-call LOW); multi-thread virtual clock.
- [x] **§4.6 Retry/Schedule (Feature #6)** — DONE. Composable IMMUTABLE
      `Schedule` (`recurs`/`exponential` + `max_attempts`/`max_delay`/`jittered`/
      `while_input`/`and_then`) + SYNC `run do … end` and ASYNC
      `Async.retry(sched, block) -> RetryFuture` drivers. Pure `.rx` over
      closures/`Duration`/`Rand`/`Async.sleep`/`include Future`, with ONE thin C
      dyn-poll step (`ruxen_retry_poll_inner` wrapping the existing
      `Future_dynamic_poll`, the `ruxen_race_step` pattern) — no new compiler
      machinery. Backoff timing tested deterministically under the §4.7 TestClock
      (pin 445: `exponential(50ms).max_attempts(5)` advanced through the backoffs,
      no real sleep). Drop-mid-backoff tears down the parked timer (pin 446).
      Pins `444`–`449`. Q47 fixed the compiler bug where a
      `Fn[() -> any Future]` closure called inside a bootstrap class made the
      bound checker reject the returned existential's own `Future` guarantee;
      the Int-ptr channel remains unchanged in this task.
      Parked LOW (for maintainer): the `schedule.c` Poll-leak inline comment is
      narrower than the actual leak (documented fully in runtime CLAUDE.md); the
      `RetryFuture.init` placeholder backoff is a tiny per-instance heap leak.

### Language features

- [x] **String-literal coercion model — `String.from("literal")` removed
      everywhere we ship (DONE 2026-06-10, `feat/drop-elaboration`).** Verified
      model: `""` is an owned `String` (lowered via `ruxen_string_from`); at a
      call site a bare literal also coerces to a `&String`/`&str` param; `&""`
      is `&&str` (NOT `&String` when annotated — bare is the idiom); `&String`
      and `&str` are distinct borrow types the unifier bridges as equivalent
      (`unify.rs:380`); `String.from(x)` is ONLY for copying a runtime borrow.
      Swept 78 tutorial sites + 2 stdlib (`test/src/runner.rx`) to bare literals;
      left every `String.from(<runtime>)` and the distinct `from_utf8`/
      `from_bytes`. Model taught in tutorial 29 (+ 02 cross-link). Pins:
      release-e2e `922_string_literal_coercion_all_positions` (RUN+stdout, owned
      + borrow), `string_literal_wrap.rs::bare_string_literal_coerces_..._all_
      positions`.
- [x] **`String.from` DELETED from the language (DONE 2026-06-11,
      `feat/drop-elaboration`).** The follow-on to the DEFERRED public-API
      decision above. The `def self.from as "ruxen_string_from"(s: &String) ->
      String` decl was removed from `library/std/string/src/string.rx`; `clone`
      is now the sole borrow→owned spelling (it shares the SAME C symbol
      `ruxen_string_from`, which also still backs the literal heap-copy
      machinery — the C runtime is untouched). Swept the remaining in-repo call
      sites (`.clone` for a `String` borrow, `.to_string` for `&str`); rewrote
      the one deliberate hold-out `116_option_ok_or` via an owned `let miss:
      String = "missing"` + `miss.clone` (still pins `ok_or`, no longer needs
      `String.from`). Dead special-cases removed: the `"from"` arm of
      `STATIC_CTORS` (`runtime_abi.rs`) and the `"from" => ruxen_string_from`
      arm of the `?T` inferred block (`lang_intrinsics.rs`). NEW seam: typeck's
      method-not-found gate (`infer/collect.rs`) now errors for `Ty::String`/
      `&str` heads (their whole surface is the `.rx` class, already consulted),
      so a `String.from(...)` call produces a clean `no method `from` on type
      `String`` diagnostic instead of leaking a `?T…` symbol. Pins:
      `stdlib_string_negatives.rs::string_dot_from_is_now_an_unknown_method`
      (clean no-such-method error), `923_clone_on_borrow_owns` (RUN+stdout
      borrow→owned `.clone`). (The mangled `String_from` entries in the
      `FRESH_ALLOC_CALLEES` ownership oracle stay — they are a defensive
      double-free-sensitive characterization table, now unreachable but harmless;
      the C symbol they co-list is load-bearing.)
- [x] **Ruby-block semantics — explicit `&block` / `yield` / `block_defined?`
      (DONE 2026-06-10, `feat/drop-elaboration`).** ADR
      `docs/decisions/ruby-block-semantics.md`. Explicit optional
      `&block: Fn[(T…) -> R]` (canonical square-bracket spelling; paren form kept
      for back-compat; `fmt` preserves both), `yield`/`yield(args)` with the
      block's value typed `R`, `block_defined?` + `block_given?` alias, optional
      blocks with a clean runtime panic on blockless `yield` (E-free, exit 101),
      single `do…end`/`{ }` attachment rule for free fns and methods, new
      diagnostic **E1119** (`&block` not last). Block slot = 8-byte
      closure-pair-pointer + null sentinel, independent of Q2. Pins: release-e2e
      908–912, `tests/ruby_block_semantics.rs`, drop-leak soundness pin.
- [x] **Ruby `alias` keyword — `alias new_name old_name` (DONE 2026-06-10,
      `feat/drop-elaboration`).** ADR `docs/decisions/alias-keyword.md`. A pure
      resolver synonym (one body, two names; zero duplicated codegen), valid in
      class/struct/enum/mixin/extension bodies (method synonym) and at
      top-level/module scope (free-fn synonym). Contextual keyword (not
      reserved). Plain + `?`/`!` names; operator aliases completed by the S4
      follow-up below (E1123 remains for unsupported operator forms). New
      diagnostics **E1120/E1121/E1122/E1123** (registered + `docs/errors/`).
      Accepted on every surface (compiler/fmt/repl/lsp/ide). stdlib sweep:
      `Array#to_a` → `alias to_a clone`. Pins: release-e2e 913–918,
      `tests/alias_keyword.rs`.
- [x] **Alias follow-up · S4 — operator aliases (`alias << push`, `alias [] get`)
      (DONE 2026-07-14, `fix/alias`).** The existing post-typeck operator
      desugars now consume the same canonical method-name map as ordinary calls.
      Supported: `+ - * / % & | ^ << >>`, unary `!`/`-@`, and `[]`/`[]=`.
      Unary `+@` remains E1123 because Ruxen has no unary-plus expression; operator
      families outside the operator-method grammar remain parser-rejected. Pins
      470–471 and `tests/alias_keyword.rs`. ADR D6.
- [x] **Alias follow-up · S4 — aliasing a mixin DEFAULT-method target (DONE
      2026-07-14, `fix/alias`).** Typeck alias registration still prefers the
      type's own canonical method, then falls back to the default-body signature
      of a mixin the type nominally includes. Required methods without a body are
      not admitted by this fallback. Pin 472 and `tests/alias_keyword.rs`. ADR D7.
- [x] **Block follow-up · S3 — closure/block captures are freed at
      closure-drop (FIXED 2026-07-14, `fix/blockdrop`).** Each closure now has
      synthesized drop glue that NULL-guards and drops provably-owned by-value
      heap captures in reverse order, then frees the captures block and closure
      pair. The caller retains ownership across the synthetic `&block` borrow.
      Capture aliases that return, enter an aggregate, reach a consuming or
      ambiguous call, or come from a nested re-capture are conservatively not
      field-dropped. Pins: `closure_and_block_captures_are_freed_at_closure_drop`
      (`allocs=3, frees=3` for both `{ }` and `do…end`),
      `closure_drop_does_not_free_a_moved_out_capture`, and release-e2e 475.
- [x] **Block follow-up · S3 — paren-less blockless call to an optional-block
      METHOD doesn't fill the block slot (FIXED 2026-06-10,
      `feat/drop-elaboration`).** `w.frame` (no parens, no block) on a method
      declaring `&block` parsed as a `FieldAccess`; that no-arg method path
      (`mir/lower/expr/field_access.rs`) did not append the `nil` block default,
      so MIR emitted one too few args and CRASHED the arity verifier
      (`__closure_*: got 1, expected 2`). FREE functions had no gap (the
      blockless `render` in pin 909 works). Fixed via the "fill defaults at MIR"
      option: the no-arg method route now appends the resolved method's trailing
      default sentinels via `Lowerer::method_trailing_default_sentinels`
      (`mir/lower/mod.rs`) — the MIR mirror of typeck's
      `append_method_default_args` the parens path already runs — so `w.frame`
      and `w.frame()` lower identically (consistent with the regular-default
      fix `autocall_uses_real_default_not_null`). Pins: release-e2e
      `921_block_optional_method_parenless` (RUN+stdout; revert CRASHES, not
      just asserts), `ruby_block_semantics.rs`
      (`parenless_blockless_method_call_fills_block_slot`,
      `explicit_block_param_on_method` extended). ADR known-limitations updated.
- [ ] **Block follow-up · Tier-2 (staged, per ADR).** `&` block-forwarding
      (`g(&block)` / anonymous `&`), `next` as block-value, `&:symbol` to-proc
      sugar, numbered params / `it`. Rejected (see ADR): non-local `return` /
      Ruby `break`-exits-yielder, `redo`, lenient arity, `instance_eval`
      self-rebinding. Nested `yield` inside a closure body errors cleanly in
      Tier 1.
- [x] **Interpolating a closure prints silent pointer garbage → now E0729
      (FIXED 2026-06-10, `feat/drop-elaboration`).** A bare `do … end` is a
      closure literal, NEVER an expression block (parser `atoms.rs`: "do…end is
      always a closure"), so `let v = do … end; puts "#{v}"` bound the
      un-invoked closure and MIR interpolation's "unknown type → Int_fmt
      (pointer-as-int)" fallback printed a raw pointer — silent garbage, and the
      `docs/tutorial/05-control-flow.md` "Blocks as expressions" section taught
      exactly this broken form. Fixed: typeck (`infer/expr.rs` Interpolation arm)
      now rejects a `Fn`/`FnMut`/`FnOnce`-typed interpolated part with **E0729**
      (`docs/errors/E0729.md`); an invoked closure's result (`#{f.()}`) and all
      ordinary values are unaffected. Tutorial section rewritten (helper-fn /
      invoke-and-format alternatives; the match-arm `-> do … end` value form is
      real and kept). Pins: `ruby_block_semantics.rs::interpolating_a_closure_
      is_e0729` + `..._invoked_closure_result_is_ok`.
- [ ] **PARKED design question — Ruby-style expression blocks.** If a block that
      is itself a *value* (multi-statement expression producing its last value)
      is ever wanted, the unambiguous spelling is **`begin … end`**, NOT
      `do … end` — `do … end` now firmly means "a block attached to a call"
      (enforced by E0729 on the silent-pointer path). Today the value-producing
      multi-statement need is met by a helper function or an `if`/`match`
      expression. No work scheduled; recorded so the spelling decision is not
      re-litigated ad hoc.

- [x] **Q34 · S2 — `ruxen fmt` drops grouping parentheses (FIXED 2026-06-10,
      `feat/drop-elaboration`).** `(rel*span + track_w/2)/track_w` →
      `rel*span + track_w/2/track_w` broke quiver's slider math. Fixed by
      re-parenthesizing operands by precedence (`formatter/prec.rs`, mirroring
      `parser::expr::infix_binding_power`); the syntax-parity harness's fmt axis
      (reparse-identity + idempotence over the whole stdlib + sibling corpus)
      also caught four sibling fmt-destructiveness bugs fixed alongside. The
      "do NOT bulk-run `ruxen fmt`" caution is LIFTED — fmt is reparse-faithful
      over all 492 corpus files. Pins: `tests/q34_fmt_grouping_parens.rs`,
      `tests/syntax_parity.rs`, release-e2e 919, ADR
      `docs/decisions/syntax-parity-harness.md`.

- [x] **Q40 · S2 — `Mutex.lock` link failure FIXED; bare-`Mutex[String]`
      set/get drop-timing edge filed separately (2026-06-11,
      `feat/drop-elaboration`).** ROOT CAUSE of the link failure was NOT a
      `Mutex[String]` mono gap — the ergonomic `lock` wrapper had NO codegen body
      for ANY `Mutex[T]` (Int link-failed identically): typeck advertised
      `Mutex.lock -> Result[MutexGuard[T], PoisonError]`
      (`method_resolvers/concurrency.rs`) but `mutex.rx` only implemented
      `lock_raw`, so `m.lock` emitted an undefined `_Mutex_lock` symbol. FIXED by
      implementing `lock`/`lock!`/`into_inner` as real `.rx` bodies in `mutex.rx`
      (layered on `lock_raw` + `is_poisoned`); `PoisonError` gained an `init` so
      `Err(PoisonError.new)` constructs. Proven RUN+stdout-correct for `&Mutex[
      String]` borrow, captured-closure, and quiver's `SharedSync`-owns-class-
      owning-`Mutex[String]` round-trip (the `ClipboardCell` shape). Pins:
      release-e2e 925/926/927. **Filed (NOT fixed here, deeper pre-existing):**
      (1) `MutexGuard` drops at FUNCTION exit not lexical BLOCK exit → two locks
      in one scope DEADLOCK (general RAII block-drop gap); (2) a heap value stored
      through a generic FFI setter (`ruxen_mutex_guard_set`) is freed by the
      caller → dangling read through a closure (the i64-stripped generic payload
      ownership gap, `State[Array]`/`Mutex[Array]` family). `Mutex[String]` is
      sound for the single-lock-per-scope / SharedSync shapes quiver uses; details
      §Q40.
- [x] **Q35 · S3 — struct/enum `include <Mixin>` satisfies generic mixin bounds
      (FIXED 2026-07-14).** Typeck's mixin collector registered inner nominal
      impls for classes but only derives for structs/enums. Their `impl_blocks`
      now populate the same nominal registry (and generic include-argument map).
      Pin `659` compiles/runs the struct + Q17 free-function path as `one=21`;
      focused typeck pins retain E1015 for a non-including struct and cover enum
      symmetry. Details §Q35.
- [x] **Q36 · S2 — `yield` with two `&var` reference args miscompiled (FIXED
      2026-07-14).** The parser treated parenthesized `yield(a, b)` as one tuple
      expression, so the synthetic-`__block` indirect call received one tuple
      slot instead of two reference slots. `yield` now parses parenthesized
      arguments with the normal call-argument parser. Release pin `929` proves
      both real fields mutate (`ui=2 root=1`); the in-process block suite also
      pins untyped explicit-`&block` parameter inference, which is working.
      Details §Q36.
- [x] **Q41 · S2 — LLVM backend can lower `DataAddr` class-info refs for
      `dispatch runtime` mixin types (FIXED 2026-07-14).** LLVM declared user
      functions but never emitted the vtable/class-info data globals, and its
      `DataAddr` arm deliberately bailed instead of taking a global address.
      The backend now mirrors Cranelift's phased lowering: it emits internal
      constant opaque-pointer arrays with method/global relocations before
      function bodies, resolves each `DataAddr`, and stores the coerced address.
      LLVM `-O2` pin `q41_runtime_mixin_class_info_lowers_links_and_runs`
      verifies, links, and executes dynamic dispatch through a future-like
      runtime mixin. This restores the optimized path for async/rondo projects;
      details §Q41.
- [x] **Q46 · S2 — a no-arg `def init` class's paren-less `.new` CRASHES
      (exit=None/segfault) when the value is moved into another class's field
      (FIXED 2026-07-14).** The paren-less `FieldAccess` constructor branch
      allocated and called `_init` without emitting the runtime-mixin class-info
      header written by the parenthesized branch; later `any Mixin` dispatch
      dereferenced slot 0 as null. Found building the §4.5 Stream adapters: a source class
      with `def init` (no params), constructed `Src.new`, runs fine alone but
      segfaults (empty stdout) when moved into an adapter's `any Stream` field.
      Fixed by initializing class info in that branch; pinned by reduced custom
      runtime-mixin case 460. Details in `dev/gui-stack-v1-issues.md` §Q46.
- [x] **Q47 · S2 — calling a `Fn[() -> any Future]` closure in bootstrap
      invalidates `any Future: Future` (FIXED 2026-07-14).** The bound checker
      treated the returned existential as a concrete type named `any Future`
      and queried the nominal/structural registry under that synthetic key.
      `some`/`any` mixin values now satisfy the exact bounds they carry. Pin
      `473` combines the bootstrap-shaped call with an unrelated stored-dyn
      poll; a focused Rust typeck pin asserts the same ordering. Details §Q47.
- [x] **Q42 · S2 — deliver a DOM event to a Ruxen handler on wasm — NOT a
      compiler bug; pure-Ruxen pattern. RESOLVED 2026-06-16 (verified by spike).**
      Spike GREEN: exported `def boot() -> Registry` (holds `handlers: Array[any
      Fn]`) + `def dispatch_event(reg: &var Registry, id, kind)` calling
      `reg.handlers.get(id)` via `f.()` + `def read_count`, compiled to wasm32 and
      run in node — JS holds the i32 `Registry` handle, calls `dispatch_event`
      twice across SEPARATE calls, `read_count` → 2, ZERO host imports. No
      `__indirect_function_table`, no `call_indirect`, no codegen edit. (`init` is
      the reserved constructor word → exported entry named `boot`.) Repro
      `tmp/spikeA/`; details §Q42 + plan RESULTS.
- [x] **Q43 · S3 — wasm single-threaded sync runtime — RESOLVED 2026-06-16.**
      Bundled single-threaded `ruxen_mutex_*`/`ruxen_sharedsync_*` in the
      `WASM_RT_C` shim (one-slot i64 boxes); quiver's `State` runs on wasm with no
      sync host imports (verified `quiver/examples/counter-dom`). wasm-only, no
      native regression. Originally filed as: Phase-0.5
      Spike C: a minimal `Mutex` compiles to wasm32 cleanly (does NOT hit the Q41
      vtable wall), but `std.sync` is pthread-backed so `mutex.c` is excluded from
      the curated wasm runtime → `ruxen_mutex_new/lock/guard_get/guard_set` are
      undefined host imports. Proven to run with a thin single-threaded JS sync
      shim (the canvas-harness approach). Fix: ship a `cfg(wasm)` no-op sync
      runtime so quiver-on-wasm is self-contained. Repro `tmp/spikeC/`; §Q43.
- [x] **Q44 · S3 — `Formatter` wasm runtime — RESOLVED 2026-06-16.** The
      `WASM_RT_C` shim now defines the mangled `Formatter_new/write_str/buffer`
      over the bundled `fmt.c`, so interpolation is self-contained on wasm
      (verified `quiver/examples/counter-dom`). Originally filed as:
      `"…#{x}"` lowers to `Formatter_new/write_str/buffer`
      (ABI: `new()->i64`, `write_str(i32,i32)->void`, `buffer(i32)->i64`); `fmt.c`
      isn't in the curated wasm runtime → undefined host imports. Host-shimmable,
      but `buffer` must return the result String IN wasm memory (JS→wasm marshalling
      gap — no exported allocator, so the shim bump-writes a high scratch region).
      Proven working in the browser counter (`quiver/examples/counter-dom`). Fix:
      bundle a `cfg(wasm)` fmt runtime, or export `ruxen_alloc` so a host shim can
      allocate safely. Sibling of Q43; §Q44.
- [x] **Q37 · S2 — generic `frame[S: Mixin]` gets a bogus `__block` when
      consumed by a binary — SAME BUG AS Q37·S1, FIXED 2026-06-10.** This was
      filed separately as the "binary-consumes-library" symptom (`could not infer
      type for parameter __block in function frame` on quiver's yield-free
      `frame[S: PaintSurface]`), but the root-cause analysis (ledger §Q37)
      confirmed it is the SAME name-keyed `yield_fns` over-attribution as S1:
      canvas's block-taking `frame` METHODS registered the bare name `frame`, so
      quiver's unrelated generic free fn `frame` inherited a phantom `__block`.
      Fixed by deciding the synthetic `__block` LOCALLY from each function's own
      body (`resolve/funcs.rs`; the name map deleted). Not a distinct open issue.
      Pin: release-e2e 920. Verified: quiver examples build again.
- [x] **Q38 · S4 — a `String` local from a BARE LITERAL was not freed at scope
      exit (leaked); `String.from("x")` was freed (FIXED 2026-06-10).** Surfaced
      sweeping `String.from("literal")` → `"literal"`; the user requires `""` to
      behave identically to `String.from`. ROOT CAUSE was type inference, not
      drop analysis: an un-annotated `let s = "x"` adopted the literal's resolver
      type `Ty::Str` (`&str`), so the local was `&str`-typed even though MIR
      lowers the literal to an owned `String` via `ruxen_string_from` — and the
      drop filter only frees `Ty::String`, so the heap copy leaked. FIXED in
      typeck (`infer/mod.rs::promote_bare_string_literal_binding`): an
      unconstrained `let` bound to a bare `StringLiteral` now binds `Ty::String`.
      Narrow (explicit `let s: &str = "x"` and call-site coercions untouched).
      The whole `.rx` corpus's `String.from("literal")` (incl. the `drop_fixtures`
      String pins) is now swept to bare literals. Pins:
      `drop_fixtures.rs::string_local_is_freed_on_scope_exit`,
      `string_literal_wrap.rs::bare_string_literal_let_binds_owned_string`.
      Details §Q38.
- [x] **Q39 · S3 — a bare `""` in a TUPLE-ELEMENT position expecting `String`
      was not coerced (stayed `&str`); broke a `(String, Bool)` return (FIXED
      2026-06-11).** Filed from rondo W21 while finishing the `String.from`
      deletion arc. The all-positions literal-coercion pin (`922`) covered
      owned/borrow/field/`Err()` slots but NOT a tuple constructor element, so
      `def f -> (String, Bool); ("", false)` typed `(&str, Bool)` and failed to
      unify; rondo spelled it `("".to_string(), false)`. ROOT CAUSE: tuple-literal
      synthesis (`infer/expr.rs`) types elements bottom-up with no expected-type
      context. FIXED via `infer/mod.rs::coerce_tuple_literal_elements` (wired at
      the return + let seams, descends if/else/block/match tails), promoting a
      bare String-literal element to owned `String` against a `String` slot —
      mirrors the `Err("msg")` precedent, no drop hazard. Narrow (only
      String-literal-in-String-slot; genuine mismatches still error). Pin:
      release-e2e `924_tuple_element_string_literal_coercion` (rondo's exact
      `(String, Bool)` shape, RUN+stdout). rondo can drop the W21 workaround.
      Details §Q39.
- [x] **`&str` removal — collapse the string-borrow type to `&String` only
      (DONE 2026-06-11, user-requested; ADR `docs/decisions/one-string-type.md`).**
      Ruxen now has exactly `String` (owned) + `&String` (borrowed). The
      `Ty::Str` variant is ELIMINATED (`hir/types.rs`), the unify bridge
      (`unify.rs:380`) and `&str→String` coerce arm deleted, the `method_home_key`
      Str arm and `&str` Display arms gone. A string literal is BORN owned
      `Ty::String` (`resolve/exprs.rs`); the Q38/Q39 owned-position promotion
      patches and the `Err("msg")` payload rewrite were removed (subsumed). The
      raw `.rodata`/FFI `char*` temps that used `Ty::Str` are now
      `Ty::RawPtr(Char)`. `str`/`&str` in a type annotation errors with new
      **E0730** (`docs/errors/E0730.md`; hint → `String`/`&String`). Stdlib swept
      (`as_str -> &String` — a borrow, drop-safe; `trim*/-> String` — owned).
      What died: the `&str`-vs-closure overload heap-corruption landmine
      (gui-stack Q1), the `&"literal"`-is-`&&str` oddity (now a clean `&String`),
      and the nested-payload double-free caution (case 116 — `ok_or("missing")`
      now builds `Result[_, String]` directly). Diagnostics/REPL `:type`/LSP/fmt
      never print `str`. Pins: `string_literal_wrap.rs`, e2e 116/643/922/923/924,
      REPL session, `drop_fixtures.rs::string_literal_and_clone_are_drop_safe`.
      Gate: `cargo test --workspace` 1983 pass; `release-e2e/run.sh` 832/832
      (parity + LSP + REPL); clippy/fmt clean. Apps to sweep post-install
      (coordinator): canvas 4 `&str` sites (`examples/demo.rx:3`,
      `src/canvas.rx:662,681`, `src/path.rx:101`); quiver 1 comment
      (`src/dsl.rx:25` — the dead landmine note); rondo 0.
- [ ] **Follow-up (one-string-type ADR): zero-copy `&String` borrow of a literal.**
      A bare string literal passed DIRECTLY to a `&String` param currently
      heap-copies (copy-everywhere) and the copy lands in a borrow-typed slot
      that nobody frees → it LEAKS (pre-existing; see next item for the root
      cause — it leaked identically under the old `&str` model). The sound fix:
      when a `StringLiteral` is lowered in a borrow-param position, emit the raw
      `.rodata` pointer (`Ty::RawPtr(Char)`, an immortal borrow) instead of
      heap-copying — zero-copy AND leak-free. Needs param-target-type-aware arg
      lowering (the "deep provenance plumbing" the ADR flagged). Pin: extend the
      drop matrix with `borrow_len("transient")` once landed.
- [x] **Discovered drop-elaboration gap: a `String` passed to a USER function's
      `&String` param suppressed the source's scope-exit free → LEAK (FIXED
      2026-06-11, `feat/drop-elaboration`).** `let owned = "x"; user_fn(owned)`
      where `user_fn(s: &String)` tainted `owned` in
      `compute_dealloc_safe_locals` because the default `callee_ownership` for a
      user fn treated every arg as consumed, dropping `owned`'s
      `ruxen_string_free`. PRE-EXISTING (it leaked identically under `&str`),
      affected every heap type through a user borrow param. FIXED by
      `mir/lower/runtime_abi.rs::user_callee_param_is_ref`: the drop pass resolves
      the callee's declared param ref-ness by NAME (class-qualified for methods,
      free-fn by base name; `fbe65da` borrow_check precedent) and skips tainting
      `&T`/`&var T` arg slots; a read-only `&self` (`Ref`) receiver is also
      untainted (the class instance no longer leaks). By-value params and
      `var self`/`consume self` receivers keep their taint (no double-free). Also
      fixed: loop-body built-in heap locals free via the shared
      `drops::heap_free_callee` — **scope-exit only, NOT at the back-edge**
      (the first cut freed at the back-edge ignoring moves: a per-iteration
      local moved OUT into an escaping collection was freed while the
      collection still held it → UAF — rondo's router read `<none>` from its
      params map, 12 failures. Corrected 2026-06-12; pin: release-e2e
      `928_loop_collection_insert_no_uaf`, the minimal reduction of rondo's
      `Route.matches`. The per-iteration leak for NON-moved loop locals is the
      accepted residual until move-aware back-edge tracking exists), and the
      `compute_dealloc_safe_locals` Assign/Copy aliasing invariant was hardened to
      strip a copied-from src's ownership even when the dest is pre-tainted
      (prevents a loop double-free). Runtime callees untouched (parity oracle
      byte-identical). Pins: `drop_fixtures.rs` borrow-call matrix (9 cases). The
      sibling **zero-copy `&String` borrow of a literal** follow-up above remains
      open (an optimization; copy-everywhere is still correct & now leak-free).
- [x] **Q32 · S3 — Q16 flat-merge of an FFI dependency broke `ruxen test` at
      link (FIXED 2026-06-10).** A consumer's test EXECUTABLE flat-merged the
      FFI dep's `src/**.rx` (incl. `lib "C"`-calling bodies) but neither
      compiled its `runtime/**.c` nor propagated its `[system_libs]` → every
      `ruxen_*` symbol undefined at link. Fix (option (b)): the test runner now
      gathers each flat-merged dep's `runtime/**.c`
      (`codegen::find_runtime_sources_in_dir`) and forwards each dep's
      `[system_libs]` (`codegen::parse_system_libs`) as `--link-arg=-l<lib>`
      (new repeatable `ruxenc` flag), mirroring exactly what `compile_project`
      gathers for a directly-declared dep; `compile_project` also gained the
      dep `[system_libs]` propagation it was missing. `src/ruxenc/test_runner.rs`,
      `src/ruxenc/compile.rs`, `src/ruxen_cli/build.rs`. Pins (staged install,
      RUN + assert): `src/ruxen_cli/tests/ffi_dep_link.rs` (FFI-dep test links +
      passes; binary declaring the dep directly has no dup-symbol; non-FFI dep
      still links). Details in `dev/gui-stack-v1-issues.md` §Q32.
- [x] **Q33 · S2 — `Float32 == <negative Int literal>` evaluated false (FIXED
      2026-06-10).** The `Compare` MIR instruction is width-blind; codegen
      coerced the rhs to the lhs's SSA type with the signedness-BLIND
      `coerce_value` (`fcvt_from_uint`), so a signed `Int(-1)` (i64 `0xFFFF…`)
      became `1.8e19` and `f == -1` was false. The ordering ops (`>=`/`<`) and
      the literal-on-the-left shape broke too; positive literals and f32==f32
      were accidentally fine. Fix: re-materialize a mismatched numeric operand
      pair to a common float width via a target-typed `Assign` before the
      `Compare` (`coerce_compare_operands` in `mir/lower/expr/binops.rs`),
      invoking codegen's Q5 signedness-aware int→float path. Backend-agnostic
      (shared MIR); mirrors Q28's `coerce_to_field_ty`. Pins (RUN + assert
      stdout): `tests/release-e2e/cases/653_f32_negative_int_literal_compare`,
      `654_enum_f32_payload_negative_compare` +
      `compiler/ruxen_core/tests/q33_negative_literal_float_compare.rs`.

- [x] **Q28 · S1 — `Float32` field/payload store-via-local → 0 / crash (FIXED 2026-06-09).**
      The struct/enum/tuple constructor lowering stored each field value
      width-blind (at the value's SSA width) into a fixed 8-byte slot; an f64
      value into an f32 field stored 8 bytes, and the f32 `GetField` read 4 →
      0. Fix: coerce each constructor arg to the FIELD's declared width via a
      target-typed `Assign` (`coerce_to_field_ty` → shared `coerce_value`
      fdemote/fpromote path) BEFORE the `SetField`, in
      `mir/lower/expr/constructors.rs` (Construct/EnumVariant/Tuple) and the
      struct auto-ctor in `method_call.rs`. Backend-agnostic (shared MIR). All
      four shapes now print 204.75; the e2e pins RUN + assert stdout. Pins:
      `tests/release-e2e/cases/650_f32_field_store_via_local`,
      `651_enum_f32_payload_via_local` (+ 647/648) and
      `compiler/ruxen_core/tests/q28_enum_float_payload.rs`.
- [x] **Q30 · S4 — `ruxen fmt` rewrote builder-closure call shapes (FIXED 2026-06-09).**
      `fmt` dropped a no-arg closure header (`{ || … }` → `{ … }`, a crash
      shape) and stripped `()` off a zero-arg call (`row_height()` →
      `row_height`, a call→identifier semantic change). Fix: a zero-param
      `ClosureExpr` always formats with an explicit `||`, and a `Call` node
      always emits its parens (it only exists when the source wrote `()`).
      `formatter/format_expr.rs`. The brace block-arg is already preserved as
      braces. Round-trip pins in `q23_fmt_nondestructive.rs`.
- [x] **Q31 · S1 — repeated `Float`-payload enum construction crashed: enum
      UNDER-ALLOCATION (FIXED 2026-06-09).** `alloc_size` sized enums to packed
      `layout.size` while codegen addresses payloads on a fixed 8-byte slot
      stride (`GetPayload` = base+8, field N at N*8), so `Move(Float32,Float32)`
      stored field 1 four bytes past the 16-byte alloc → heap-metadata
      corruption → crash on the next float `malloc` (hence ≥2 constructions; Int
      payloads on 8-byte slots survived). Fix: slot-round enum allocations to
      `8 + widest_variant_field_count*8` in `mir/lower/emit.rs` (no drops/codegen
      change; enum dealloc was already sound — 3 allocs/3 frees). Pins run+assert
      stdout: `tests/release-e2e/cases/652_enum_float_payload_double_construct`,
      `compiler/ruxen_core/tests/q31_float_enum_payload_drop.rs`,
      `drop_fixtures.rs::q31_…_no_leak`. Unblocks canvas `Int`→`Float32` coords.
- [x] **Q16 · S4 — dependency symbols invisible to library/`check`/`test` builds
      (FIXED).** Library (`compile_piece`), `check`, and `ruxen test` now
      flat-merge dependency `src/**.rx` via the shared `build::gather_dep_sources`
      + `build::resolve_dep_source_dirs`, the same mechanism binary builds use.
      `rondo`/`quiver` can now unit-test against their own public API directly.
      Pin: `src/ruxen_cli/tests/dep_visibility.rs`.
- [x] **Q17 · S4 — generic-free-fn / mixin-bound monomorphization for consumer types.**
      ✅ FIXED for generic FREE FUNCTIONS (feat/drop-elaboration, 2026-06-10).
      Re-scoped empirically: post-Q16 this is a single-unit MIR-lowering gap, not
      cross-package. A generic free fn over a mixin now monomorphizes per concrete
      implementor (incl. generic-calling-generic via a worklist fixpoint), so a
      consumer binary can define a SECOND `PaintSurface` implementor and call the
      dep's generic against both — quiver's framework cap is lifted. Design:
      `docs/decisions/q17-cross-package-monomorphization.md`. Q17b completes the
      staged generic-method surface below. Still out of scope: true rlib/separate-
      compilation generics (Q16's flat-merge is the model).
- [x] **Q17b · S4 — generic METHOD over a mixin (FIXED 2026-07-14).** Generic
      methods declared directly inside class/struct/enum receivers now use the
      Q17 demand-driven path: collect concrete method-argument instantiations,
      suppress the placeholder-prone opaque body, emit specialized self-bearing
      bodies (`Frame_measure_scaled__mono__Wide`), and redirect resolved method
      calls. Release-e2e pin `474` compiles+runs two implementors with distinct
      results; the focused Q17 suite also pins struct and enum receiver owners.
      Unsupported generic-calling-generic across method boundaries errors clearly
      at lowering and never falls through to a placeholder or missing symbol.
- [ ] **Q2 · S1 — `Option[any Fn[...]]` class field returns garbage.** ⏸ Deferred
      pending the closure representation redesign; apps work around with index-into-pool arrays.
- [ ] **Q22 — closure captures are pointer-copies.** ✅ Audited 2026-06-08 post-P0.2:
      **SOUND, not a UAF** (S4, not the feared S1). Now that `Drop` runs, a captured
      class local is permanently tainted out of the drop set by the capture's
      `SetField` (`drops.rs:728` / `closure.rs:136-150`), so it is **not freed** and the
      stored closure's pointer does not dangle — it **leaks** instead (lives to process
      exit). Does NOT gate the next Drop increment. Open item is the owning-capture /
      keep-alive design (Q4 prerequisite) so escaped handles are freed deterministically
      instead of leaked. Verdict + mechanism in `dev/gui-stack-v1-issues.md` §Q22.
- [x] **Q23 · S4 — `ruxen fmt` is destructive** ✅ FIXED 2026-06-08. (a) Nested
      class/struct/enum/impl/mixin method `##` docs are now emitted
      (`format_func_with_leading_comments`). (b) The shared parser accepts a clean
      top-level expression statement (`TopLevelItem::Expr`) so `ruxen fmt`
      round-trips `Tester.describe` test files. The later implicit-main feature
      made the same shape executable on direct compile; E0728 now applies only
      when an explicit `main` is mixed with loose statements. Pin:
      `compiler/ruxen_core/tests/q23_fmt_nondestructive.rs`.
- [x] **Q24 · S4 — stale incremental cache replays false move/borrow diagnostics**
      ✅ FIXED 2026-06-08. The cache key's toolchain component was just
      `CARGO_PKG_VERSION`, invariant across a `--from-source` rebuild, so stale
      objects (with the old compiler's borrow behaviour) were replayed. `compile.rs`
      now folds a `toolchain` fingerprint (exe path + size + mtime) into the cache
      flags, and `CacheKey` gained a `flags` component so the per-object key
      reflects it. Pin: `cache_key_differs_on_flags`.
- [x] **Q25 · S1 — `Hash.key?`/`get` on an EMPTY hash SEGFAULTS**; `&Hash`/`&Set`
      params unsound. ✅ FIXED 2026-06-08. (a) The `string_keys` tristate (-1 unset)
      was C-truthy, so empty-table lookups `strcmp`'d an int key as a `char*` —
      `hash.c` now tests `> 0`. (b) The `&Hash`/`&Set` "unsoundness" was a free-fn
      false-positive E1118 on `&Hash[K,V]` (the `Hash → Hashable` alias); now both
      free fns and methods accept the sound `&Hash[K,V]` collection ref (like
      `&Array[Int]`), and the bare `&Hash` mixin ref is still rejected. Pins:
      `tests/release-e2e/cases/617_*`, `618_*`, `619_*` +
      `compiler/ruxen_core/tests/q25_hash_set_soundness.rs`.
- [x] **Q26 · S1 — capturing closure stored under a `&var *self` reborrow loses its
      captures** ✅ FIXED 2026-06-08. Root cause: a closure nested inside another
      closure's body never re-captured a variable the OUTER block captured
      (`capture_map` was excluded from the visible-defs set). Closure lowering now
      treats `def_to_local ∪ capture_map` as visible and fills re-capture slots from
      the enclosing captures pointer. Unblocks reactive `dyn_text`/`button` children
      in quiver containers. Pins: `tests/release-e2e/cases/615_*`, `616_*` +
      `compiler/ruxen_core/tests/q26_nested_closure_capture.rs`.
- [x] **Q28 · S1 — enum variant `Float`/`Float32` payloads (claimed miscompile)**
      ✅ FIXED / already sound 2026-06-09 (feat/drop-elaboration). The
      `canvas/src/event.rx` `Int`-coordinate TODO ("return to Float32 payloads once
      enum float payloads work") is STALE — like Q22, the note outlived the defect.
      Enum `Float`/`Float32` payloads round-trip exactly (named + positional, single +
      double, mixed with `Int` variants, through fns + Arrays, sub-pixel values). The
      typed MIR `SetField`/`GetField` slot path stores/loads each float at its own
      width; an f32 literal is `coerce_value`-narrowed to f32 in `Assign` before the
      constructor. Fixed as a side effect of Q5 + the case-218 / `1b6ced0` float-codegen
      work. No code change; pinned as a regression guard. Pins:
      `tests/release-e2e/cases/647_enum_float32_payload`, `648_enum_float_mixed_payload`
      + `compiler/ruxen_core/tests/q28_enum_float_payload.rs`. Affected site
      `canvas/src/event.rx` can revert to `Float32` (canvas owner).
- [x] **Q29 · S1 — borrowed `&String` into a `lib "C"` FFI call (claimed wrong
      pointer)** ✅ FIXED / NOT-A-BUG 2026-06-09. A borrowed `&String` forwards the
      correct data pointer + recoverable length today: a Ruxen `String` IS a bare
      NUL-terminated `char*` (no length header), and `MirInst::Ref` is by-value, so the
      `char*` passes through unchanged; C recovers length via `strlen`. The old "char
      count / wrong pointer" claim described the legacy `measure_text_n_raw(n: Int)`
      workaround, not `&String`. Evidence: a borrowed `&String` through `include?`/
      `find`/`replace`/`starts_with` returns exact byte-offset/length-sensitive results.
      Pins: `tests/release-e2e/cases/649_ffi_borrowed_string_arg` +
      `compiler/ruxen_core/tests/q29_ffi_borrowed_string.rs`. Canvas deviation note +
      redundant `measure_text_n_raw` fallback can be reverted (canvas owner).

## Open now — pre-flight P0s (`requirements/ROADMAP.md`)

The P0 list blocks tiers downstream. **It predates recent compiler work and its
file paths reference the old `crates/` layout — re-audit against the current tree
before picking one up** (several may be partially landed; check `CHANGELOG.md`).
Highest-leverage, still-relevant:

- [x] **P0.2 — real `Drop` elaboration (RESOLVED; premise stale).** `MirInst::Drop`
      is an inert marker; `insert_drops` (`mir/lower/drops.rs`) emits per-type free +
      user-destructor calls at scope exit, gated by `compute_dealloc_safe_locals` so a
      moved / returned / FFI-moved value is never dropped. Proven: `drop_fixtures.rs` +
      `user_drop_runs.rs` 19/19, `outstanding == 0`. ADR: `docs/decisions/drop-elaboration.md`.
      Remaining tier1_04 work (drop flags for conditional moves, drop-on-unwind,
      Copy/Clone) is separate and still open.
- [ ] **P0.5 — unresolved generic method calls fall back to `ruxen_noop_passthrough`**
      (silent no-op). Violates the "no silent no-op" rule.
- [ ] **P0.15 — variance rules encoded as comments only; no fixtures prove them.**
- [ ] Remainder (P0.1, P0.3–P0.14): triage against current tree.

## GUI-stack critical path (what `canvas`/`quiver` are waiting on)

The GUI stack (`../canvas`, `../quiver`) has shipped its first vertical slice
but its next cycles are gated here. Prioritized by blast radius:

- [x] **P0.2 — real `Drop` elaboration (RESOLVED).** Heap-owning locals
      (Class/Struct/Enum + String/Array/Map/Set) are freed and user `def drop`
      destructors run at scope exit, in reverse declaration order, exactly once;
      moved / returned / FFI-moved values are never dropped (no double-free /
      use-after-free). The no-GC deterministic-teardown value prop is realized for
      the normal-scope-exit case. Both backends honour it (the free is a MIR `Call`,
      so Cranelift and LLVM behave identically; `MirInst::Drop` itself is an inert
      marker). Leak-audit pin: `drop_fixtures.rs` asserts `outstanding == 0` across
      19 cases. ADR: `docs/decisions/drop-elaboration.md`. **Still open:** drop flags
      for conditionally-moved locals, drop-on-unwind (panic=abort only today),
      Copy/Clone mixins.
- [x] **Q17 — generic free-fn/method mixin-bound monomorphization (FIXED).** A
      generic free function over a mixin now
      monomorphizes per consumer-defined implementor, so quiver's
      single-`PaintSurface` cap is lifted — multiple paint backends now compile +
      run. Generic-calling-generic handled via a worklist fixpoint. **Unblocks:**
      multiple paint backends + clean L1/L2 generic seams. Q17b now covers direct
      generic methods on class/struct/enum receivers as well.
- [x] **Q16 — dependency symbols in library/`check`/`test` builds (FIXED,
      feat/drop-elaboration).** Dep `src/**.rx` is now flat-merged into library
      (`compile_piece`), `check`, and `ruxen test` builds via the shared
      `build::gather_dep_sources` + `build::resolve_dep_source_dirs` — not just
      binary builds. Symbols enter by source flat-merge (one object, no
      extern-rlib link), so binary builds are unchanged and there is no
      duplicate-symbol risk. Pins: `src/ruxen_cli/tests/dep_visibility.rs`,
      `test_runner` synth-order unit test. ADR:
      `docs/decisions/q16-dep-symbols-in-lib-check-test-builds.md`.
      **Unblocks:** quiver/rondo unit-testing their own public API instead of
      through a sibling binary.
- [x] **Q26 — closure capture lost under `&var *self` reborrow.** ✅ FIXED
      2026-06-08. **Unblocks:** reactive `dyn_text`/`button` children inside quiver
      `Row`/`Col` containers (the widget library's core; previously only static
      `text` children worked in containers). S1.
- [ ] **Enum float payloads + FFI `&String` pointer bugs.** canvas works around
      both (pointer coords forced to `Int`; `measure_text` forwards a char count
      not the string). File/repro as `Q##` if not yet tracked, then canvas
      reverts its deviations. **Unblocks:** correct float event coords + real
      text advance widths.

## Long-horizon

Tiers 1–5 in [`requirements/ROADMAP.md`](requirements/ROADMAP.md): stdlib
completeness & concurrency/async/drop (T1) → type-system features (T2) →
tooling/DX incl. LSP, debugger, docgen, incremental compile (T3) → ecosystem,
package manager, cross-compile, WASM, stable ABI (T4) → spec & stability,
language reference, edition/deprecation, error-code registry (T5). Gate is
`prompts/v1/25_v1_release_checklist.md`.

Forward-looking analysis (not yet committed tasks): [`decisions/zio-comparison-concurrency-adoption.md`](decisions/zio-comparison-concurrency-adoption.md)
ranks seven async-concurrency capabilities distilled from the Ruby-vs-Scala/ZIO
comparison — cancellation + runtime-dispatched `Drop`, structured task scopes,
`Race`, async `Queue`, `Stream`, `Schedule`, and a virtual `TestClock` — each with
rondo-grounded rationale, market impact, and the maintainer decisions they gate
(async methods on classes, `segment_cfg` widening, cross-thread reactor wake).

## How to update this file

- Land a fix → check its box here (or remove it), add the `CHANGELOG.md` entry,
  pin the `tests/release-e2e/cases/` case, and flip the status in the source doc
  (`gui-stack-v1-issues.md` / `requirements/ROADMAP.md`).
- A sibling app hits a new compiler quirk → add a `Q##` entry to
  `dev/gui-stack-v1-issues.md` with a repro, then surface it here under "Open now".
