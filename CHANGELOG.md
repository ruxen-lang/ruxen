# Changelog

All notable changes to this project will be documented here. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once 1.0.0 ships.

## [Unreleased]

### Added
- **WASM target (tier 4.03): `ruxen compile --target wasm32-unknown-unknown`.**
  Produces a valid `.wasm` module via the LLVM backend + `wasm-ld` (no libc,
  no C runtime — a reactor module). Every top-level free `def` becomes a
  host-callable wasm **export** under its source name, via the LLVM
  `export_name` function attribute (decouples the export from Ruxen name
  mangling — spec §9 Q6's preference). A wasm build does NOT bootstrap the
  hosted stdlib (the no_std reality), which also sidesteps the LLVM backend's
  not-yet-emitted vtable/class_info globals. **Headline bar passes:** a Ruxen
  `def add` compiles to a `.wasm` that runs in Node.js with `add(2,3)===5`
  asserted (`examples/05-wasm/`, `scripts/wasm_verify.sh`, pin
  `tests/wasm_codegen.rs`). New `MirProgram::wasm_exports`, `ResolvedTarget::
  is_wasm`, `object::emit_wasm_module`. Needs a toolchain built with
  `--features llvm` (`LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18`); the
  default build stays Cranelift. Staged remainder (filed in
  `docs/decisions/phase4-no-std-wasm.md`): `wasm32-wasi`, a bundled allocator
  for String/Array exports, `wasm_import`/host imports, `std.wasm`, the
  per-function `wasm_export "custom"` rename directive, and a browser harness.
- **LLVM verification lane brought up (tier 4.03 prereq).** The codegen module
  CLAUDE.md's "never built / no llvm-config" notes were stale: LLVM 18.1.8 is
  installed; `LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 cargo build
  --features llvm` builds (the only bit-rot was one dead import, removed).
  `ruxen_cli`'s `llvm` feature now propagates to `ruxenc/llvm` (a latent
  non-exhaustive-match bug that only surfaced when the variant was enabled
  without the arm). The default toolchain build is unchanged (Cranelift).
- **Cross-compilation (tier 4.02): `--target <triple>`.** `ruxen compile`
  (ruxenc) and `ruxen build/run/check` accept a target triple; default = host
  (byte-identical to before — no `--target` changes nothing). New
  `codegen::target` module: alias-normalize + parse triples (explicit alias
  table — `target-lexicon` 0.13 parses `x86_64-macos` lossily, contra the
  spec), Cranelift `isa::lookup` (enabled `all-native-arch`), LLVM
  `TargetTriple::create`, and a linker matrix: host→`cc`; Darwin→Darwin →
  `cc -arch`; Linux w/ cross-gcc → that gcc; Linux w/o → a **two-stage Docker
  link** (object emitted locally, stdlib runtime compiled + linked in a
  target-native container). Per-target output dir `target/<triple>/<profile>/`;
  per-target runtime compiled FOR THE TARGET (no host/cross cache poisoning,
  pinned). `ruxen target list/add/remove` (add/remove = loud `Err`, not a
  silent no-op — prebuilt-fetch deferred). `ruxen run --target <non-host>`
  errors (no emulator). §5.8 backend-compat error for wasm-on-cranelift.
  **Both acceptance bars pass:** `aarch64-unknown-linux-gnu` runs in a
  `linux/arm64` container; `x86_64-apple-darwin` runs under Rosetta
  (`scripts/cross_verify.sh`). Docs: `docs/CROSS_COMPILE.md`, ADR
  `docs/decisions/cross-compilation-linker-matrix.md`.

### Fixed
- **`Mutex.lock` / `lock!` / `into_inner` now LINK and run (Q40).** typeck
  advertised the ergonomic poison-surfacing `Mutex.lock ->
  Result[MutexGuard[T], PoisonError]` (and siblings) via a hardcoded
  `method_resolvers/concurrency.rs` arm, but `library/std/sync/src/mutex.rx` only
  implemented the `_raw` FFI — so any real `m.lock` typechecked then emitted an
  undefined `_Mutex_lock` symbol and link-failed, for EVERY `Mutex[T]` (not just
  `Mutex[String]`; the original report's "direct links" was `lock_raw`). The
  wrappers are now real `.rx` bodies layered on `lock_raw` + `is_poisoned`;
  `PoisonError` gained an `init` so the `Err(PoisonError.new)` arm constructs.
  Proven RUN-correct for `&Mutex[String]` borrow, captured-closure, and the
  `SharedSync`-owning-`Mutex[String]` round-trip (quiver's `ClipboardCell`
  shape). Pins: release-e2e 925/926/927.
  - Two deeper, pre-existing drop-timing issues the link fix surfaced are FILED,
    not fixed (gui-stack §Q40): (1) `MutexGuard` drops at function exit, not
    lexical block exit, so two locks in one scope deadlock (general RAII gap);
    (2) a heap value stored through the generic `ruxen_mutex_guard_set` FFI is
    freed by the caller → dangling read across a closure (the i64-stripped
    generic-payload ownership gap). `Mutex[String]` is sound for the
    single-lock-per-scope / SharedSync shapes quiver actually uses.
- **Drop elaboration: a heap value passed to a USER function/method's `&T`
  borrow param no longer leaks.** A `String` (or `Array`/`Hash`/`Set`/class)
  passed to `def f(s: &String)` was tainted by the conservative user-callee
  default in `compute_dealloc_safe_locals` (every arg of an unknown callee
  treated as consumed), so the source local's scope-exit `ruxen_string_free` was
  suppressed and the source LEAKED. Pre-existing; it leaked identically under
  the old `&str` model and was surfaced by the one-string-type drop matrix.
  - **Fix:** `mir/lower/runtime_abi.rs::user_callee_param_is_ref` resolves the
    callee's declared param ref-ness by NAME (class-qualified for methods,
    mirroring the `fbe65da` borrow_check precedent — the HIR method DefId can be
    `UNRESOLVED_DEF`). For a user callee the drop pass now skips tainting any arg
    whose declared param is `&T`/`&var T`; a read-only (`&self`/`Ref`) method
    receiver is likewise treated as a borrow (the receiver instance no longer
    leaks). By-value (consuming) params and `var self`/`consume self` receivers
    KEEP their taint — no double-free. Runtime callees are untouched (the
    classified `callee_ownership` tables are unchanged; the parity oracle is
    byte-identical).
  - **Loop bodies:** a built-in heap local (`String`/`Array`/`Hash`/`Set`)
    declared inside a `while`/`loop`/`for` body frees via the type-correct
    helper (extracted `drops::heap_free_callee`, shared with scope-exit drop)
    at SCOPE EXIT only. (The first cut additionally freed at the loop
    back-edge, which was UNSOUND — it ignored moves, so a per-iteration local
    moved out into an escaping collection (rondo's `Route.matches` inserting
    path-param keys into its captures Hash) was freed while the collection
    held it: a use-after-free reading back `<none>`, 12 rondo test failures.
    Corrected; pinned by release-e2e `928_loop_collection_insert_no_uaf`. The
    per-iteration leak for non-moved loop locals is the documented residual
    until move-aware back-edge tracking exists.)
  - **Aliasing invariant hardened:** `compute_dealloc_safe_locals` now strips a
    copied-from alloc-rooted `src`'s ownership unconditionally (even when the
    `dest` is already tainted, e.g. a loop body-local pre-zeroed by
    `prepend_zero_init`), preventing a double-free where the producing
    `ruxen_string_from` temp and the loop-freed local aliased the same pointer.
  - Pins: `drop_fixtures.rs` borrow-call matrix (9 cases — borrow→source freed
    once; by-value→kept tainted/no double-free; multiple borrow args;
    borrow-then-use-after; borrow-in-loop; `&Array`/`&self`-class; mixed args).

### Removed
- **`&str` — removed from the language; one string type pair only.** Ruxen now
  has exactly `String` (owned) and `&String` (borrowed). The distinct `Ty::Str`
  (`&str`) primitive is gone (ADR: `docs/decisions/one-string-type.md`). It was
  a parallel spelling of the same wire value — at the C ABI a `String` and the
  old `&str` are the *same* representation (a bare null-terminated `char*`, no
  length header) — so the second type added only confusion and drop-safety
  hazards.
  - **A string literal is born owned `Ty::String`.** Into an owned slot it
    heap-copies (`ruxen_string_from`, already drop-safe); a literal also
    satisfies a `&String` param (the call site borrows it). The Q38/Q39
    owned-position promotion patches (`promote_bare_string_literal_binding`,
    `coerce_tuple_literal_elements`) and the `Err("msg")` payload rewrite are
    deleted — there is no `&str` to promote.
  - **`str` / `&str` in a type annotation is an error — new code E0730** (hint:
    "use `String` (owned) or `&String` (borrowed)"). `docs/errors/E0730.md`.
  - **The `&str`-vs-closure overload heap-corruption landmine is dissolved**
    (gui-stack Q1): with one string borrow type there is no `&str` arm to
    collide with a closure arm in overload selection.
  - **The nested-payload double-free caution dies** (release-e2e case 116):
    `opt.ok_or("missing")` now builds `Result[_, String]` directly — no `&str`
    payload over `String` storage, no storage-vs-payload drop mismatch.
  - The raw `.rodata`/FFI `char*` temporaries that `Ty::Str` used to type are
    now `Ty::RawPtr(Char)` (`*Char`) — `Copy`, never dropped, the correct type
    for an un-owned C string pointer.
  - Stdlib swept: `as_str` is now `-> &String` (it returns the receiver's own
    buffer — a borrow, never freed; declaring `-> String` would double-free the
    source); `trim`/`trim_start`/`trim_end` are `-> String` (they `malloc` a
    fresh owned buffer). No C runtime change. Diagnostics / REPL `:type` / LSP
    hover / `ruxen fmt` never print `str` — they say `String` / `&String`.
  - Pins: `string_literal_wrap.rs` (all-positions, typeck), release-e2e `116`,
    `643`, `922`, `923`, `924`, the REPL `:type "hi" => String` session, and the
    new `drop_fixtures.rs::string_literal_and_clone_are_drop_safe` leak/double-
    free pin. Full `cargo test --workspace` (1983 pass) + `release-e2e/run.sh`
    (832/832 incl. parity + LSP + REPL) + clippy/fmt clean.
- **`String.from` — deleted from the language.** The `String.from(s: &String)
  -> String` static method is gone. The string model is now uniform and needs
  no conversion constructor:
  - a string literal is an **owned `String`** (`let s = "x"` owns + drops; a
    bare literal also satisfies a `&String` parameter at a call site);
  - **`&x`** borrows;
  - **`x.clone`** copies a borrow (or any `String`) to a fresh owned `String`
    — this is the sole borrow→owned spelling that `String.from` used to serve.
    (`b.to_string` is the equivalent `&String`→owned spelling.)

  `clone` keeps backing onto the SAME C runtime symbol `ruxen_string_from`,
  which also drives the string-literal heap-copy machinery — the **C runtime is
  unchanged**; only the surface method spelling was removed. Calling the deleted
  `String.from(...)` now produces a clean `no method `from` on type `String``
  diagnostic (typeck now treats an unknown method on a `String` head as a
  hard error, since its entire surface is the `.rx` `class String`) rather
  than silently resolving or leaking a `?T…` symbol into codegen. All in-repo
  call sites were swept to `.clone` / `.to_string` / owned let-bindings; the
  four sibling repos were already `String.from`-free. Pins:
  `stdlib_string_negatives.rs::string_dot_from_is_now_an_unknown_method`,
  release-e2e `923_clone_on_borrow_owns` (RUN+stdout). Ledger: gui-stack §Q38
  (the bare-literal model this completes).

### Added
- **Syntax-parity harness** (ADR `docs/decisions/syntax-parity-harness.md`).
  Enforces the USER invariant that the compiler, `ruxen fmt`, the REPL, the
  LSP, and the IDE never diverge on Ruxen syntax — *"ruxen syntax must be 100%
  available on every package we deliver."* Two axes:
  - **Per-surface conformance** over a single auto-discovered corpus (the
    compiler's `tests/release-e2e/cases/` + `library/std/` + `examples/` + the
    read-only sibling repos `canvas/quiver/rondo` `src/`, 491 files): the
    compiler lexes+parses each; `ruxen fmt` re-parses to a structurally
    identical AST (span-blind, import-order-tolerant) and is idempotent; the
    REPL's `parse_repl_input` accepts every batch-accepted top-level item kind;
    the LSP/IDE `analyze` parses everything the compiler does.
  - **Structural pins**: a compile-time exhaustiveness guard that breaks the
    build when a new `TopLevelItem`/`MixinItem`/`ImplItem` variant lands without
    a parity decision, and an explicit intentional-divergence allowlist (the
    parser-accepts-but-compile-rejects E0728/E0607 class).
  - Delivery: `compiler/ruxen_core/tests/syntax_parity.rs`,
    `src/ruxen_ide/tests/syntax_parity_ide.rs`, and a new `parity` phase in
    `tests/release-e2e/run.sh` (driving the shipped `ruxen fmt` binary; wired
    into `PHASES=all`).
- **Ruby-style `alias` keyword** (ADR `docs/decisions/alias-keyword.md`).
  `alias new_name old_name` (space form, Ruby keyword style) gives an existing
  method or free function a second name as a **pure resolver synonym** — both
  names resolve to ONE body, with zero duplicated codegen and zero extra call
  frame.
  - Valid as an item in `class` / `struct` / `enum` / `mixin` / `extension`
    bodies (a method synonym scoped to the type) and at top level / in a
    `module` (a free-function synonym). `alias` is a CONTEXTUAL keyword (not
    reserved) — existing identifiers named `alias` keep working.
  - Plain names AND `?`/`!` names work (`alias member? include?`). Operator
    aliases (`alias << push`) are staged for Tier 2 with a clear **E1123**.
  - Diagnostics: **E1120** (unknown target), **E1121** (alias cycle), **E1122**
    (name collides with an existing def / self-alias), **E1123** (operator alias
    staged). Each registered + documented under `docs/errors/`.
  - Accepted on every toolchain surface: compiler, `ruxen fmt` (byte-stable
    round-trip), the REPL (`parse_repl_input` routes the contextual keyword),
    and the LSP/IDE (no spurious diagnostics).
  - **stdlib sweep:** `Array#to_a` is now `alias to_a clone` (was a duplicate
    FFI decl binding the same C symbol) — one fewer method body. The
    return-type-differing families (`get`/`get_mut`/`get_var`) and the
    `&str`-bridge-entangled `to_s`/`to_string` were deliberately left as FFI
    decls (a pure synonym cannot express a differing signature or the dual
    method-home routing). Pins: `tests/release-e2e/cases/913–918`,
    `compiler/ruxen_core/tests/alias_keyword.rs`.
- **Ruby-block semantics** (ADR `docs/decisions/ruby-block-semantics.md`).
  A function/method may declare an explicit, optional trailing block parameter
  with the `&` sigil and the canonical square-bracket signature spelling:
  `def render(x: Int, &block: Fn[(Int) -> nil])`. The paren spelling
  `Fn(T…) -> R` is also accepted; `ruxen fmt` preserves whichever was written
  (carried by a semantically-inert `bracketed` flag on `TypeExpr::Function`).
  - `yield` / `yield(args)` invokes the block; for an explicit `&block` decl
    `yield`'s value IS the block's declared return type `R` (`let r = yield(4)`).
    The block is also a normal callable value in the body (`block.(args)`).
  - `block_defined?` (and the alias `block_given?`) is a `Bool` builtin, true
    iff the caller passed a block — the user's conditional-render pattern.
  - Every `&block` is OPTIONAL: calling without a block is legal; the slot is a
    null closure-pair-pointer sentinel; reaching a `yield` with no block is a
    clean runtime panic naming the function (exit 101), never a segfault.
  - Trailing `do…end` and `{ }` blocks attach identically as the implicit last
    argument, the SAME rule for free functions and methods.
  - **New diagnostic E1119**: `&block` must be the last parameter
    (`docs/errors/E1119.md`).
  - Representation is independent of the broken `any Fn` enum-payload path
    (Q2 stays open and untouched). Pins: `tests/release-e2e/cases/908–912`,
    `tests/ruby_block_semantics.rs`, `drop_fixtures.rs::block_capturing_heap_value_runs_soundly`.

### Fixed
- **A bare `""` in a TUPLE-ELEMENT position expecting `String` now coerces to an
  owned `String`** (ledger Q39; rondo W21). The all-positions literal-coercion
  work covered owned/borrow/struct-field/`Err(...)`-payload positions, but a
  bare string literal in a tuple constructor stayed `&str` — so a `def f ->
  (String, Bool); ("", false)` typed `(&str, Bool)` and failed to unify with the
  declared return. Fixed in typeck (`infer/mod.rs::coerce_tuple_literal_elements`,
  wired at the return + `let` seams, descending if/else/block/match tails): a
  bare `StringLiteral` element against a `String` tuple slot is re-typed
  `Ty::String` (the literal already lowers through `ruxen_string_from` to an
  owned heap copy, matching codegen — same precedent as the `Err("msg")`
  payload coercion). Narrow: only a String-literal in a String slot is rewritten,
  so a genuine `(Int, Bool)` vs `(String, Bool)` mismatch still errors. Pin:
  release-e2e `924_tuple_element_string_literal_coercion` (rondo's exact
  `(String, Bool)` shape, RUN+stdout). Lets rondo drop the `("".to_string(),
  false)` workaround for bare `("", false)`.
- **A `String` local bound from a BARE LITERAL leaked (not freed at scope
  exit); now owns + drops like an explicitly-owned binding** (ledger Q38). An un-annotated
  `let s = "hello"` adopted the literal's resolver type `Ty::Str` (`&str`), so
  the local was `&str`-typed even though MIR lowers the literal to a heap-owned
  `String` via `ruxen_string_from` — and drop elaboration only frees
  `Ty::String`, so the heap copy leaked (`let s: String = "hello"` was
  freed because it forced `s: String`). Fixed in typeck
  (`infer/mod.rs::promote_bare_string_literal_binding`): an unconstrained `let`
  bound to a bare `StringLiteral` now binds an owned `String`. Narrowly scoped —
  an explicit `let s: &str = "x"` keeps the borrow, and call-site/argument
  coercions are untouched (a bare literal still coerces to a `&str`/`&String`
  parameter). This is what made the bare-literal corpus sweep (and the later
  `String.from` deletion, see Removed) leak-free. Pins:
  `drop_fixtures.rs::string_local_is_freed_on_scope_exit` (fixture is now
  `let s = "hello"`), `string_literal_wrap.rs::bare_string_literal_let_binds_
  owned_string`. (Follow-up filed in TASKS: collapse `&str` into `&String` so
  there is one owned string type + its borrow.)
- **`Err("literal")` in a `-> Result[T, String]` function now coerces the
  payload to `String`** instead of building `Result[T, &str]` and failing to
  typecheck. The `Err` constructor took its payload type straight from the
  argument (`&str` for a literal) and never coerced against the enclosing
  return's error type, so `Err("msg")` only worked when the `Ok` branch pinned
  the Result concretely; with an inferred `Ok(a / b)` branch it failed. Typeck
  (`infer/expr.rs` Result `Err` arm) now coerces a bare string-literal error
  payload to the expected `String`. PRE-EXISTING gap exposed by the
  `String.from("literal")` sweep (the old `Err(String.from("msg"))` papered over
  it) — fixed forward, not reverted. Pin: release-e2e
  `922_string_literal_coercion_all_positions` (uses the inference-order-sensitive
  `Ok(a / b)` shape).
- **Interpolating a closure / `Fn` value printed a silent pointer instead of
  erroring → new diagnostic E0729.** A bare `do … end` is a closure literal,
  never an expression block (parser rule: "do…end is always a closure"), so
  `let v = do … end; puts "#{v}"` bound the un-invoked closure and MIR
  interpolation's "unknown type → `Int_fmt` (pointer-as-int)" fallback printed a
  raw pointer address — silent garbage that shipped teaching material
  (`docs/tutorial/05-control-flow.md`) actually taught. Typeck now rejects a
  `Fn`/`FnMut`/`FnOnce`-typed interpolated part with **E0729**
  (`docs/errors/E0729.md`, registered); an invoked closure's result (`#{f.()}`)
  and all ordinary values are unaffected. The tutorial section was rewritten.
  Pins: `ruby_block_semantics.rs::interpolating_a_closure_is_e0729`,
  `..._invoked_closure_result_is_ok`.
- **MIR: a paren-less, blockless call to an optional-`&block` METHOD crashed
  the arity verifier** (Ruby-block-semantics ADR D1/D5; the blocks feature's
  filed Tier-1 known limitation). `w.frame` (no parens, no block) on a method
  declaring `&block` parses as a `FieldAccess` whose no-arg method route did not
  append the `nil` block default, so MIR emitted one too few args and crashed
  (`__closure_*: got 1, expected 2`); `w.frame()` (parens) worked because the
  parens `MethodCall` path gets typeck's `append_method_default_args`. Fixed by
  the MIR mirror: the no-arg method route in `mir/lower/expr/field_access.rs`
  now appends the resolved method's trailing default sentinels via the new
  `Lowerer::method_trailing_default_sentinels` (`mir/lower/mod.rs`) — a null
  closure-pair-pointer (`Literal::Int(0)`) per unsupplied defaulted trailing
  param — so `w.frame` and `w.frame()` lower IDENTICALLY (consistent with the
  earlier regular-default fix `autocall_uses_real_default_not_null`). Pins:
  release-e2e `921_block_optional_method_parenless` (RUN+stdout; a revert
  CRASHES at MIR, not just an assert miss), `ruby_block_semantics.rs`
  (`parenless_blockless_method_call_fills_block_slot`,
  `explicit_block_param_on_method` extended with `w.build`).
- **Resolve: a yielding/`&block` method poisoned an unrelated same-named free
  function with a phantom `__block`** (ledger Q37, S1). The synthetic-`__block`
  decision keyed off a bare-function-name map (`yield_fns`), so a block-taking
  method `frame` made an unrelated generic free fn `frame` inherit a `__block`
  it could never infer (`could not infer type for parameter __block`) — which
  broke every quiver example binary once canvas gained its `frame` methods. The
  decision is now made LOCALLY from each function's own body
  (`resolve/funcs.rs`); the name-keyed map + its populator are removed. Pin:
  release-e2e 920.
- **`ruxen fmt` was destructive in four ways** (all surfaced by the new
  syntax-parity harness; ADR `docs/decisions/syntax-parity-harness.md`, ledger
  Q34):
  - **Q34 — dropped grouping parentheses**, silently changing arithmetic
    (`(a + b) / c` → `a + b / c`). The parser keeps no paren node, so the
    formatter now RE-DERIVES grouping by operator precedence
    (`formatter/prec.rs`, mirroring `parser::expr::infix_binding_power` as the
    single precedence source). Pin: `tests/q34_fmt_grouping_parens.rs`.
  - **Zero-arg method call → field access** (`s.bytes()` → `s.bytes`). A
    `MethodCall` and a `FieldAccess` are distinct AST nodes; the formatter now
    always emits the call `()`.
  - **Method visibility section dropped** — a `private`/`protected` method
    round-tripped as `public`. Class/struct/enum bodies now emit
    `private`/`protected`/`public` section markers as visibility changes.
  - **`async` modifier dropped** (`async def f` → `def f`). Now emitted.
- **Borrow checker: false `value used after move` (E1001) on an owned value
  passed to a `&T` / `&var T` parameter.** `check_method_call` / `check_fn_call`
  decided move-vs-borrow purely from the ARGUMENT's own type, so an owned value
  auto-borrowed into a reference parameter — `s.include?(needle)` where
  `include?(needle: &String)` and `needle` is an owned `String` — was recorded as
  a MOVE, and a second use of the value (`s.find(needle)`) was rejected. The fix
  also consults the callee's declared PARAMETER type (resolved by name, since the
  `lib`-declared method's HIR DefId can still be `UNRESOLVED_DEF` at borrow-check
  time): a value passed to a reference parameter is an auto-borrow, never a move.
  This was latent — the existing Q29 pin (`tests/q29_ffi_borrowed_string.rs`)
  validated the fixture through Lexer→Parser→typeck→MIR→codegen but **skipped
  `borrow_check`**, so it never saw the false positive; the full-pipeline CLI
  (`ruxen compile`) and the release-e2e harness did. Added a borrow-check pin that
  runs the full pipeline including `borrow_check` and asserts zero errors, closing
  the coverage gap. Pin: `tests/release-e2e/cases/649_ffi_borrowed_string_arg` +
  `borrowed_string_arg_passes_borrow_check_with_no_false_move`.
- (Q17) A generic **free function bound by a mixin** (`def paint_all[T:
  Paintable](s: &var T, …)`) could not be monomorphized for a SECOND mixin
  implementor: the bound method call inside its body (`s.fill_rect(…)`) mangled
  to the bound-placeholder callee `T: Paintable_fill_rect`, which link-failed.
  The single-implementor case was masked because mixin dispatch devirtualized to
  the sole impl — exactly why quiver's framework was capped at ONE `PaintSurface`
  backend (`RecordingSurface`). **Empirical re-scope:** after Q16's dep-source
  flat-merge this is NOT a cross-package problem (it reproduces in a single
  file); it is a MIR-lowering gap. Fix (`mir/lower/monomorphize.rs`): a new
  demand-driven pass collects each concrete instantiation of an eligible generic
  free fn (recovered by unifying declared param types against the call's actual
  arg types), emits one specialized body per instantiation
  (`paint_all__mono__TallySurface`) via the existing `subst_type_params_in_func`,
  and redirects call sites (`fn_call.rs::fn_mono_callee`). Generic-CALLING-generic
  is handled by a worklist FIXPOINT that re-scans each substituted body. The
  opaque body of every eligible generic free fn is suppressed (it could only emit
  placeholders); the single-implementor case monomorphizes to one concrete copy,
  byte-equivalent to the old devirtualize path. Backend-agnostic (shared MIR).
  **Staged remainder:** a generic METHOD over a mixin (a generic `def` inside a
  class) is not yet monomorphized — it now produces a CLEAR lowering error, never
  a placeholder symbol. Quiver's paint pass is entirely generic FREE functions,
  so the framework is unblocked. Design:
  `docs/decisions/q17-cross-package-monomorphization.md`. Pins (COMPILE + RUN +
  assert stdout): `src/ruxen_cli/tests/cross_package_mono.rs` (staged-install,
  two-package, binary + `ruxen test`, asserts `dep=20 mine=9` per-implementor),
  `compiler/ruxen_core/tests/q17_generic_fn_mixin_mono.rs`, and release-e2e cases
  `655`–`658`.
- (Q33) Comparing a float value against a **negative Int literal** miscompiled.
  `f32 == -1` evaluated **false** even when the value was exactly `-1.0`, and the
  breakage extended past equality: `f >= -1` was false, `f < -1` was true,
  `Float` (f64) vs a negative Int broke identically, and the literal-on-the-LEFT
  shape (`-1 == f`) broke symmetrically. Root cause: the `Compare` MIR
  instruction is width-blind, and codegen coerced the rhs to the lhs's SSA type
  with the signedness-BLIND `coerce_value` (`fcvt_from_uint`), so a signed
  `Int(-1)` (i64 `0xFFFF_FFFF_FFFF_FFFF`) became `1.84e19` and the `fcmp` was
  false; positive literals and f32==f32 were accidentally correct. Fix:
  re-materialize a mismatched numeric operand pair to a common float width via a
  target-typed `Assign` BEFORE the `Compare` (`coerce_compare_operands` in
  `mir/lower/expr/binops.rs`), invoking codegen's Q5 signedness-aware int→float
  path (`fcvt_from_sint` for a signed source) — exactly as a `let`-bound `as
  Float32` cast already does. Mirrors Q28's `coerce_to_field_ty`. Backend-agnostic
  (shared MIR), so Cranelift and LLVM agree; int-only and equal-type pairs pass
  through untouched (zero extra instructions on the hot matched-width path). Pins
  (COMPILE + RUN + assert stdout): `tests/release-e2e/cases/653_f32_negative_int_literal_compare`,
  `654_enum_f32_payload_negative_compare` (the canvas `Scroll(-1, 3)` shape), and
  `compiler/ruxen_core/tests/q33_negative_literal_float_compare.rs`.
- (Q32) A flat-merged FFI dependency's C runtime is now linked into
  executable-producing builds. Q16 flat-merges a path-dependency's `src/**.rx`
  (incl. `lib "C"`-calling bodies) into the consumer's `ruxen test` executable,
  but neither its `runtime/**.c` objects nor its `[system_libs]` reached the link
  line → `Undefined symbols: _ruxen_*` at link (the `ruxen_canvas_*` shape quiver
  hit). Fix (option (b) from the filing): the test runner now compiles each
  flat-merged dep's `runtime/**.c` (`codegen::find_runtime_sources_in_dir`) and
  forwards each dep's `[system_libs]` (`codegen::parse_system_libs`) as
  `--link-arg=-l<lib>` — a new repeatable `ruxenc` flag mirroring `--runtime-c=`
  — exactly mirroring what `compile_project` gathers for a directly-declared
  dep. `compile_project` (the `ruxen build` binary path) also gained the dep
  `[system_libs]` propagation it was silently missing (`collect_system_lib_flags`
  only walks the stdlib root). `src/ruxenc/src/test_runner.rs`,
  `src/ruxenc/src/compile.rs`, `src/ruxen_cli/src/build.rs`. Pins (staged install,
  real compile + link + RUN): `src/ruxen_cli/tests/ffi_dep_link.rs` — an FFI dep
  used only through `tests/**.rx` links + passes (4*10+1 == 41); a binary
  declaring the same dep directly builds + runs with no duplicate-symbol; a
  non-FFI dep still links. The Q16 `dep_visibility.rs` suite stays green.
- (Q28, REOPENED → real fix) `Float32` struct field / enum payload / tuple slot
  stores from a non-inline value miscompiled to **0** (and an uncast f64 local
  into an f32 payload could crash). The constructor lowering stored each field
  width-blind — at the value's own SSA width — into the field's fixed 8-byte
  slot, with no coercion to the field's declared type, so an f64 value (a bare
  `120.5` literal or any `Float` local) stored 8 bytes and the f32 `GetField`
  read 4 → 0. The inline `120.5f32` literal, `expr as Float32` cast, and a
  `Float32` fn-param worked only because those paths already produced an
  f32-typed SSA value before the store — which is why the earlier
  inline-literal-only audit wrongly called it sound. Fix: coerce each
  constructor arg to the FIELD's declared width via a target-typed `Assign`
  (`coerce_to_field_ty` → the shared `coerce_value` fdemote/fpromote/fcvt path)
  BEFORE the width-blind `SetField`, with field types from
  `lookup_construct_field_types` (struct/class) / `lookup_variant_field_types`
  (enum) / the tuple `Ty`. Applied in `mir/lower/expr/constructors.rs`
  (Construct/EnumVariant/Tuple) and the struct auto-constructor in
  `mir/lower/expr/method_call.rs`. Backend-agnostic (shared MIR lowering), so
  Cranelift and LLVM agree. All shapes now compute 204.75; the uncast f64 local
  auto-narrows at the field instead of crashing. The e2e pins now COMPILE + RUN
  the binary and assert exact stdout (the prior 647/648 pins passed while real
  codegen was wrong because they used only inline f32 literals and never RAN the
  load-from-local shape). Pins: `tests/release-e2e/cases/650_f32_field_store_via_local`
  (struct, all four shapes), `651_enum_f32_payload_via_local` (enum payload,
  load-from-local) + `647`/`648` +
  `compiler/ruxen_core/tests/q28_enum_float_payload.rs`. `canvas/src/event.rx`
  can now revert to `Float32` coordinates (canvas owner). Repro matrix:
  `tmp/test-cache/q28-f32-field-store-matrix.md`.
- (Q30) `ruxen fmt` no longer rewrites builder-closure call shapes into a
  crashing form. It dropped a no-arg closure header (`{ || App.build(…) }` →
  `{ App.build(…) }`, a brace block that re-parses ambiguously — a documented
  GUI-stack crash shape) and stripped `()` off a zero-arg call (`row_height()` →
  `row_height`, a call→identifier semantic change). Fix
  (`formatter/format_expr.rs`): a zero-param `ClosureExpr` always formats with an
  explicit `||` (the AST can't distinguish it from a no-pipe `{ … }`, and `||`
  is always a legal idempotent header), and a `Call` node always emits its parens
  (it only exists when the source wrote `()`). The inner brace block-arg is
  already preserved as braces (the claimed `do…end` conversion did not
  reproduce). Round-trip pins in
  `compiler/ruxen_core/tests/q23_fmt_nondestructive.rs`; `ruxen fmt` is safe to
  run on the GUI stack again.
- (Q31) Constructing a `Float`/`Float32`-payload enum variant **two or more times
  by value in one function** no longer crashes. Root cause was an enum
  **under-allocation**, not a drop double-free: `alloc_size` (`mir/lower/emit.rs`)
  sized an enum to its packed `layout.size`, but codegen addresses an enum payload
  on a fixed 8-byte slot stride (`GetPayload` = base+8, field N at N*8), so
  `Move(Float32,Float32)` stored field 1 four bytes past the 16-byte allocation →
  heap-metadata corruption → fault on the next float `malloc` (which is why it
  needed ≥2 float constructions and Int payloads survived). Fix: slot-round enum
  allocations to `8 + widest_variant_field_count*8`. The enum dealloc path was
  already sound (3 allocs / 3 frees). Pins RUN + assert stdout / clean exit:
  `tests/release-e2e/cases/652_enum_float_payload_double_construct`,
  `compiler/ruxen_core/tests/q31_float_enum_payload_drop.rs`, and
  `drop_fixtures.rs::q31_…_no_leak` (asserts `ruxen_alloc_outstanding == 0`).
  Unblocks canvas reverting event coordinates to `Float32`.
- (Q29) Verified NOT-A-BUG + pinned: a borrowed `&String` (owned by the caller)
  passed into a `lib "C"` FFI function forwards the correct data pointer and a
  recoverable length. A Ruxen `String` IS a bare NUL-terminated `char*` (no
  length header; `library/std/string/runtime/string.c`), and `MirInst::Ref` is
  by-value in both backends, so the `char*` passes through unchanged and the C
  side recovers the length via `strlen`. The old ledger / canvas ROADMAP claim
  ("forwards a char count, not the string; a borrowed `&String` passes the wrong
  pointer") described the LEGACY `measure_text_n_raw(n: Int)` char-count
  workaround, not `&String` itself. Evidence: a borrowed `&String` threaded
  through pointer/length-sensitive `String` FFI (`include?`/`find`/`replace`/
  `starts_with`) returns exact byte-offset and length results. Pins:
  `tests/release-e2e/cases/649_ffi_borrowed_string_arg` +
  `compiler/ruxen_core/tests/q29_ffi_borrowed_string.rs`. The canvas deviation
  note and the now-redundant `measure_text_n_raw` fallback can be reverted
  (canvas owner).
- (Q16) Dependency package symbols are now visible to LIBRARY builds,
  `ruxen check`, and `ruxen test` — not just binary builds. Previously only
  the binary path (`compile_project`) flat-merged a dependency's `src/**.rx`
  into the consuming compilation unit; library builds (`compile_piece`),
  `check`, and the test runner saw none of a path-dependency's symbols, so a
  library could not `use` a dependency type in `src/lib.rx` or in a
  `tests/**.rx` file (the reason quiver/rondo had to test their public API
  through sibling binary crates). The dep-source flat-merge is now a shared
  helper (`build::gather_dep_sources`) reused by all four build kinds, with a
  shared `build::resolve_dep_source_dirs` for `check`/`test`. Soundness:
  dependency symbols still enter by SOURCE flat-merge (one object, one
  definition of every symbol), never by extern-rlib link — so there is no
  duplicate-symbol/double-link risk, and binary builds are byte-for-byte
  unchanged. The test runner resolves deps in `ruxen_cli` (where the resolver
  lives) and threads the dirs into `TestOptions::dep_source_dirs`, since
  `ruxenc` only dev-depends on `ruxen_cli`. Design:
  `docs/decisions/q16-dep-symbols-in-lib-check-test-builds.md`. Pins:
  `src/ruxen_cli/tests/dep_visibility.rs` (two-package fixture — a `dep-color`
  library exposing `struct Color`, a `consumer` library that `use`s it in
  `src/lib.rx` and in `tests/color_test.rx`; `build`/`check`/`test` all green)
  and `test_runner::tests::synthesise_merges_dependency_source_before_project_and_main`.
- (Q24) The incremental build cache (`ruxen build`/`test`) is now keyed on the
  actual TOOLCHAIN IDENTITY, not just `CARGO_PKG_VERSION`, so a stale object can
  no longer be replayed after the compiler changes — which had surfaced false
  `E1001`/`E1009` move/borrow diagnostics with bogus spans (and masked correct
  ones), most visibly after a `ruxen upgrade --from-source` rebuild at the same
  version, or after an embedded-stdlib `.rx`/`.c` change. `compiler_version()`
  is derived only from the crate version + schema tag, so neither of those
  bumps it; the cache key (and the manifest's flags) now also fold a
  `toolchain` fingerprint (the running compiler binary's path + size + mtime),
  forcing a recompile that re-runs the new compiler's borrow/move analysis and
  re-emits fresh diagnostics. `CacheKey` gained a `flags` component (it already
  carried the backend / opt-override / project `runtime/*.c` fingerprint via the
  manifest header, but the per-object key ignored it). `src/ruxenc/src/cache/
  hash.rs`, `src/ruxenc/src/compile.rs`. Pin: `cache_key_differs_on_flags`.
- (Q23a) `ruxen fmt` no longer STRIPS `##` doc comments from methods nested
  inside a class/struct/enum/impl/mixin body. `format_program` emitted leading
  comments only for TOP-LEVEL items; nested methods were formatted by direct
  `format_func_def` calls that bypassed that path, silently dropping every
  doc comment (e.g. all 86 `##` docs on `canvas/src/canvas.rx`). A new
  `format_func_with_leading_comments` emits them at each nested method site,
  mirroring the existing class-body `lib "..."` FFI-def doc handling. Idempotent.
  `compiler/ruxen_core/src/formatter/format_items.rs`.
- (Q23b) `ruxen fmt` no longer errors at `1:1` on a top-level
  `Tester.describe(...) do … end` file (every `tests/*.rx`). The SHARED parser
  (`parse_top_level_item`) now accepts a clean top-level expression statement as
  a `TopLevelItem::Expr`, so the formatter round-trips test files instead of
  refusing them. The DIRECT compile path still rejects a top-level statement
  with the new E0728 ("wrap it in `def main`"); `ruxen test` is unaffected (it
  hoists items + wraps statements in a synthesised `def main` before compiling).
  New error doc `docs/errors/E0728.md`. Pins:
  `compiler/ruxen_core/tests/q23_fmt_nondestructive.rs` (5 cases incl. an
  idempotence check and a "top-level garbage still errors" negative).
- (Q25a) `Hash.key?`/`Hash.get` and `Set.include?` on an EMPTY hash/set no
  longer SIGSEGV. The runtime keeps a tristate `string_keys` flag (-1 unset /
  0 int keys / 1 string keys) resolved on the first insert; the hash/equality
  predicates tested it with plain C truthiness, and the unset sentinel -1 is
  truthy — so a lookup before any insert took the string path and `strcmp`'d an
  integer key as a `char*`, dereferencing a small bogus address (e.g.
  `(char*)9`). The predicates now test `string_keys > 0`, defaulting an
  unresolved table to raw-bits hashing (a string-keyed table always has the
  flag set to 1 by its first insert before any lookup). `library/std/hash/
  runtime/hash.c`. Pins: `tests/release-e2e/cases/617_*` (hash), `618_*` (set).
- (Q25b) A `&Hash[K, V]` / `&Set[T]` by-ref parameter now resolves
  consistently in free-fn and method position. Previously a free fn rejected
  `&Hash[Int, Int]` with a false-positive E1118 (the TEC-13 `Hash → Hashable`
  alias made the bare name resolve to the static-dispatch `Hashable` mixin),
  while a method accepted it — and the empty-hash segfault above made that path
  look like a miscompile. `&Hash[K,V]` is a sound pointer-to-struct param,
  exactly like the widely-used `&Array[Int]`, so a generic-args-bearing
  collection builtin in `&Name[..]` position now falls through to ordinary
  collection-ref resolution in both positions. The bare `&Hash` / `&Set` (no
  args = the `Hashable` mixin) is still rejected at compile time.
  `compiler/ruxen_core/src/resolve/types.rs`. Pins:
  `tests/release-e2e/cases/619_*` + `compiler/ruxen_core/tests/q25_hash_set_soundness.rs`.
- (Q26) A capturing closure nested inside another closure's body now keeps
  its captures, including when stored through a `b.(&var *self)` reborrow.
  The nested closure's free-variable analysis (`mir/lower/captures.rs`) only
  consulted the enclosing frame's `def_to_local`, so a variable captured by
  the OUTER block (which lives in `capture_map`, not a local) was never
  re-captured: the nested closure got a NULL captures pointer and read the
  value as slot garbage (`box.call0` printed 1 instead of 43; SIGSEGV for a
  captured class handle). Closure lowering now treats `def_to_local ∪
  capture_map` as the visible set and fills a re-capture slot by reading the
  value out of the enclosing captures pointer (through the cell when the
  enclosing capture is `ByRef`). The rare doubly-nested *mutate-an-outer-
  by-value-capture* shape is rejected with a clear lowering error rather than
  miscompiled. Unblocks reactive `dyn_text`/`button` children inside quiver
  `Row`/`Col` containers. Pins: `tests/release-e2e/cases/615_*`, `616_*` +
  `compiler/ruxen_core/tests/q26_nested_closure_capture.rs`.
- A same-name method overloaded on `&str` vs `any Fn[...]` now dispatches
  to the overload whose parameter type matches the call-site argument,
  independent of declaration order. The MIR symbol selector
  (`method_signature_accepts_args`) was missing the by-value-arg →
  `&T`-parameter coercion arm that the typeck selector already had, so a
  `"static"` literal (`Ty::Str`) matched neither overload's strict check
  and fell through to the arity-only fallback, binding the FIRST-declared
  overload. With the closure overload declared first, `add("static")` was
  mis-dispatched to it and `f.()` ran on a string pointer (crash / heap
  corruption). The MIR selector now mirrors the typeck one.
- A value-returning function that ends in an `if let` whose arms all
  `return` (e.g. `if let Some(f) = self.cb; return f.(); end; return 0`)
  no longer fails Cranelift verification. The implicit fallthrough block
  synthesised after the `if let` emitted a valueless `return` in a
  function whose signature declares a return value, tripping the verifier
  ("arguments of return must match function signature"). Codegen now emits
  a placeholder of the declared return type for such (unreachable)
  valueless returns.
- Struct inline-method bodies are now type-checked. Previously `typeck::infer`
  skipped them (`HirItem::Struct(_)`), so a `self.<field>` read inside a struct
  `def` kept `field_idx = 0` and an unresolved (`Infer`) result type — codegen
  then loaded the FIRST field as a raw `i64` regardless of which field was
  named. Reading a non-first integer field returned the wrong value, `UInt8`
  fields all returned field 0's byte, and `Float`/`Float32` field reads failed
  Cranelift verification (`i64` result vs `f32`/`f64` signature). Enum inline
  methods (same `§3.4a` surface) are now inferred and validated alongside
  structs.
- Derived `hash_code` for a struct with `Float`/`Float32` fields no longer
  emits invalid IR (`bxor.i64` fed an `f64`). Float fields are now
  bit-reinterpreted to a same-width integer (new `MirInst::FloatToBits`) before
  the FNV mix.
- A zero-argument `def self.X` static on a `struct`/`enum` now dispatches
  correctly at the call site. `is_user_static_method` only recognised
  `DefKind::Class`, so a struct static (`C3.white`) was mis-classified as an
  instance call and a phantom `self` (constant `0`) was prepended — tripping
  the Cranelift verifier (`got 1, expected 0`). It now also matches
  `DefKind::Struct`/`DefKind::Enum`.
- Static methods on a `struct`/`enum` now resolve their return type during
  inference. `typeck`'s method selection only fired `select_class_method` for
  `Ty::Class`, so a struct static (`C4.white()`, `Color.rgb(...)`) fell through
  to the lenient fresh-var path and its result type stayed `?T` — breaking any
  chained `.field`/`.method` and any un-annotated `let` bound to the result
  (notably inside closure bodies, which lower to standalone functions). The
  selector now covers `Ty::Struct`/`Ty::Enum` too.
- An inline `(small_int as WiderInt) << N` term no longer silently contributes
  the wrong value. The `Cast` MIR lowering passed the inner value through
  unchanged, so an integer cast nested inside a `<<`/arithmetic expression ran
  at the SOURCE width and Cranelift masked the shift amount
  (`(1u8 as UInt32) << 16` became 0). Integer→integer casts now materialise a
  value at the target width (matching the existing let-bound coercion), so
  inline bit-packing (`(a << 24) | (b << 16) | …`) produces the correct result.

### Changed
- **Dropped `String.from("literal")` from everything we ship; documented the
  string-literal model.** A bare string literal is already an owned `String`
  (lowered through `ruxen_string_from`), and at a call site it coerces to a
  `String`, `&String`, or `&str` parameter as the position needs — so
  `String.from` on a literal was pure noise. Swept **78** tutorial sites
  (`docs/tutorial/**`) and the **2** stdlib sites
  (`library/std/test/src/runner.rx`: `get(&String.from(&"…"))` → `get("…")`,
  `Err(_) -> String.from(&"…")` → `Err(_) -> "…"`) to bare literals. Left every
  `String.from(<runtime value>)` (the genuine borrow→owned copy) and the
  distinct `String.from_utf8`/`from_bytes`. The verified model is now taught in
  `docs/tutorial/29` (and cross-linked from `02`): `""` = owned `String`;
  `&""` = `&&str` (NOT `&String` when annotated — write the bare literal);
  `&String` and `&str` are distinct borrow types the unifier bridges as
  equivalent; `String.from(x)` is only for copying a runtime borrow. Regression
  pins (so the swept corpus across four repos can't silently break): release-e2e
  `922_string_literal_coercion_all_positions` (RUN+stdout, owned + borrow
  directions), `string_literal_wrap.rs::bare_string_literal_coerces_to_string_
  in_all_positions`.
- **Tutorial 05 "Blocks as expressions" rewritten.** It taught
  `let v = do … end` as producing a value — but a bare `do … end` is a closure
  literal, so that printed a pointer (now caught by E0729, see Fixed). The
  section is now "Multi-statement `match` arms" built around the working
  `-> do … end` arm form (verified to run), with the broken pattern shown only
  as a labeled error and a helper-fn alternative; the `begin … end` future
  spelling is parked in TASKS.

## [0.1.0] - 2026-05-30

### Fixed
- Cross-compilation for `aarch64-unknown-linux-gnu` (and other targets using
  older `cross` sysroots) no longer fails with `sys/random.h: No such file or
  directory`. The shared runtime header no longer includes `<sys/random.h>`
  (glibc ≥ 2.25); secure entropy in the `rand` runtime now invokes the
  `getrandom(2)` syscall directly with a `/dev/urandom` fallback.
- Release build for `x86_64-apple-darwin` no longer fails its installed-binary
  tests with `found architecture 'arm64', required architecture 'x86_64'`.
  GitHub's macOS runners are arm64 and the Intel `macos-13` image is retired,
  so that target is a cross-compile; the release workflow now builds the
  artifact with `cargo build --target` and skips the native-only tests for it
  (mirroring the `aarch64-unknown-linux-gnu` cross build).

### Added
- **Prebuilt C runtime archive.** The stdlib C runtime is no longer recompiled
  on every AOT build: a prebuilt `libruxenrt.a` ships to `~/.ruxen/lib/` and is
  auto-discovered relative to the `ruxen` binary (override with
  `RUXEN_RUNTIME_AR`). When no archive is found the compiler transparently
  falls back to compiling the runtime `.c`. NOTE: the final link still invokes
  `cc` — full `cc`-free linking is separate, deferred work. Also fixes installed
  / `--from-source` toolchains that previously shipped no runtime sources and
  could not link AOT binaries, and a stale `library/runtime/` path in the
  release packaging that produced an empty `lib/` payload.
- **`std.regex` — PCRE2-backed regex package.** New `library/std/regex/` with
  `class Regex`, `class Match`, `class RegexError`. PCRE2 10.44 is vendored
  under `library/std/regex/runtime/pcre2/` and compiled into `libruxenrt.a`
  alongside the package's `runtime/regex.c` C wrapper (no `system_libs` entry
  needed). First-class `/pat/flags` literal syntax disambiguated from division
  via the standard JS/Ruby positional rule. New `~=` operator at the equality
  precedence tier (`s ~= regex` → `Bool`; `regex.find(s)` → `Option[Match]`).
  Surface: `is_match`, `find`, `scan`, `replace`, `replace_all`, `split` on
  `Regex`; `matched`, `start_pos`, `end_pos`, `group`, `named`, `groups`,
  `named_groups` on `Match`. Flag set: `i m s x` (semantic); `g` accepted as a
  no-op. Compile-time pattern validation via PCRE2 at typeck (E1704). New
  error codes E1700/E1701/E1702/E1703/E1704 with long-form docs under
  `docs/errors/`. Seven e2e fixtures (`900_…` through `906_…`). Spec:
  `docs/superpowers/specs/2026-05-29-std-regex-design.md`.

### Fixed
- **Overloaded `drop` (and `display`/`init`) no longer demands an explicit
  return type.** The resolver renames overloaded methods to
  `<name>__overload<N>`; the "public function must have an explicit return
  type" check matched the bare name, so an overload-renamed void method (e.g.
  `drop__overload1400`) skipped the Unit default and spuriously errored. The
  check now matches the base name.
- **Shared signature renderer for IDE display.** Hover and signature help no
  longer hand-roll signature strings (which disagreed with each other and with
  `ruxen fmt` — e.g. `def f()` vs `def f`, `async def f()`). Both now call the
  new `ruxen_core::formatter::render_fn_signature`, so the IDE can never drift
  from the formatter's signature shape.
- **IDE completion keywords come from the lexer.** Completion's keyword list
  was a hardcoded copy missing ~26 real keywords (`unless`, `where`, `unsafe`,
  `const`, …) and wrongly including non-keywords (`and`/`or`/`not`). It now
  uses the lexer's canonical `KEYWORDS`, guarded by a consistency test.
- **Cross-binary parser/formatter/IDE parity.** Several constructs the
  language supports were rejected or mangled by some front-end consumers but
  not others. Since the lexer/parser/typeck/formatter are single shared
  implementations in `ruxen_core`, each was one shared-code fix:
  - Doc comments (`##`) are now accepted before FFI `def`s inside `lib`
    blocks, and at end-of-file / in doc-only source files (previously
    `expected def in lib block` / `expected top-level declaration, found Eof`).
  - `match` is now usable as a method/FFI def name (it is the canonical regex
    method), joining `var`/`some`/`any` as contextual keywords.
  - `ruxen fmt` no longer silently **deletes** `##` doc comments attached to
    FFI defs inside `lib` blocks (top-level and class-body).
  - IDE hover renders a no-parameter `def` without parens (`def f`), matching
    `ruxen fmt`, instead of `def f()`.
  - The IDE/LSP diagnostic source label is now `ruxen` (was the deprecated
    `ruxenc`).
  - New guard `compiler/ruxen_core/tests/parser_feature_parity.rs` asserts
    every shipped `library/std/**` and `examples/**` `.rx` parses.

### Added
- **Type-directed auto-call of function references (Ruby-style
  ergonomics).** A bare reference to a named nullary function/method used
  in a value position whose expected type is *not* a function type is now
  auto-called: `let argv = args` binds `args()`'s result, so `argv.len`
  resolves. A `Fn`-typed context (annotation or `Fn`-typed parameter)
  suppresses the rewrite and references the function instead
  (`let f: Fn() -> Array[String] = args`). A bare reference to a function
  that *requires* arguments reports the new **E0726** with both escape
  routes (call it, or annotate a `Fn` type). Implemented as a contextual
  rewrite in `typeck` (`auto_call_fn_reference`), so MIR/codegen and the
  IDE/LSP analysis (which share `typeck::type_check`) pick it up
  automatically. Fixes the previously-broken `examples/01-cli-utility`.
- **Universal `to_s` method on every type (Ruby convention).** Scalar
  primitives (`Int`, `USize`, `Float`, `Bool`, `Char`, `String`, `&str`)
  stringify via the existing `ruxen_*_to_string` runtime helpers. User
  classes / structs / enums get a `to_s` that returns the *same* `String`
  as string interpolation `"#{obj}"` — the MIR method-call lowering routes
  `obj.to_s()` through the identical display dispatch (derive-Debug
  structs/enums, user `impl Display`, …). A user-defined `to_s` method
  takes precedence over the synthesized default.
- **Numeric conversion methods `Int.to_f()` and `Float.to_i()`.** Ruxen
  performs no implicit `Int`↔`Float` coercion (see E0707), so these are
  the supported way to cross the integer/float boundary: `to_f` widens an
  `Int` to a `Float`, `to_i` truncates a `Float` toward zero to an `Int`.
  Backed by new runtime helpers `ruxen_int_to_f` (i64→f64) and
  `ruxen_float_to_i` (f64→i64) in `library/std/string/runtime/string.c`,
  wired through the typeck method table, the Cranelift ABI table
  (`runtime_sigs.rs`), the LLVM declarations, and the
  `lang_intrinsics::runtime_name` symbol mapping (the missing mapping was
  why `a.to_f()` previously died as `can't resolve symbol Int_to_f`).
- **REPL+runtime: replay-suppression flag for non-idempotent runtime
  helpers (refactor phase 3 — Path A).** New
  `library/std/core/runtime/repl_replay.c` hosts a thread-local
  `ruxen_repl_is_replaying` flag plus `ruxen_repl_set_replaying` /
  `ruxen_repl_get_replaying` accessors declared in
  `library/std/core/runtime/runtime.h`. Every non-idempotent runtime
  function — the `ruxen_puts` / `ruxen_print` / `ruxen_eputs` /
  `ruxen_print_int` / `ruxen_print_float` family, `ruxen_command_status`
  / `ruxen_command_output` / `ruxen_process_exit`, `ruxen_fs_write` /
  `_remove_file` / `_create_dir*` / `_rename` / `_copy` /
  `_remove_dir_all` / `_write_atomic` / `_symlink`, `ruxen_file_create`
  / `_append` / `_open_options` (write modes) / `_write*` / `_flush`,
  `ruxen_tcp_listener_bind` / `_accept`, `ruxen_tcp_stream_connect` /
  `_read` / `_write`, `ruxen_async_tcp_listener_bind`,
  `ruxen_stdout_print` / `_println` / `_write_str` / `_flush`, etc. —
  early-returns a benign value (`Ok(())` / a pre-closed sentinel /
  `void`) when the flag is set. Idempotent reads
  (`ruxen_fs_read*` / `_metadata` / `_canonicalize` / `_read_link` /
  `_read_dir`, env getters, read-only `ruxen_file_open`) ignore the
  flag and always execute — needed for correct replay of let-RHS
  expressions whose values depend on the world. The REPL's stdout
  capture shims in `src/ruxen_repl/src/capture.rs` (the JIT overrides
  the puts/print symbols with these shims, so the C-side gate alone
  wouldn't catch them) re-check the same flag via
  `ruxen_repl_get_replaying`. The REPL wraps the replay portion of
  every input's wrapper body in synthetic
  `__repl_set_replaying(1)` / `__repl_set_replaying(0)` calls,
  declared in the existing `repl_slot_lib` FFI block. Side effects
  fire exactly once per input.

### Changed
- **REPL state model: chronological `session_var_mutations` replaces
  `all_statements` + capture-buffer line-count diff.** The REPL no
  longer accumulates a separate `prev_captured_output` and diffs
  cumulative stdout by line count — the runtime suppression flag
  handles duplicate-puts cleanly. `ReplSession::all_statements`
  becomes `session_var_mutations` (the same chronologically-ordered
  Vec, narrowed in name only); `ReplSession::prev_captured_output`
  is removed. The new `last_output: String` field stashes the
  current input's captured stdout so headless test harnesses
  (`tests/state_persistence::run_session`) can snapshot what each
  input wrote. Net effect: the 5 baseline REPL-parity failures
  (508/534/536/727/727b) drop to 3 (508/727/727b — 508 fails for an
  unrelated subprocess-vs-parent stdout-buffering interleave, the
  two 727s are async-executor territory queued for phase 4); the
  ~30 mutation-with-puts fixtures stay green; the two
  previously-`#[ignore]`'d `single_execution` tests are now green
  unconditionally.

### Fixed
- **REPL parity: 727_async_tcp_echo + 727b_async_tcp_read_timeout
  removed from `REPL_KNOWN_SKIP`.** Both fixtures now PASS the REPL
  parity sweep end-to-end. The final unblock was filtering
  slot-backed `let` bindings (and same-target assignments) out of
  the replay stream in `collect_replay_statements`: the wrapper's
  synthetic slot-load prefix is already the source of truth for
  those values, and replaying the original let-RHS would re-execute
  side-effecting initializers AND lexically shadow the slot-loaded
  binding with a fresh — often wrong — value. For 727 specifically,
  `let handle = Thread.spawn_raw(...)` was replaying on every
  subsequent input, the second pthread spawn's bind hit
  `EADDRINUSE`, server_loop returned 0, the lexical rebind made
  `handle = 0`, and the replayed `if handle == 0; ...; return; end`
  exited the wrapper before the user's actual input could run.
  With the filter, the slot load of `handle` (the original valid
  pthread_t) survives; replay no longer re-spawns; the rest of the
  fixture (sleep, `client_flow`, the if-else echo print, `JoinHandle.join_raw`)
  runs normally. Required four coordinated changes: the filter
  itself, a probe typecheck in `eval_statement::Let` so the slot
  is registered BEFORE the first `build_program` call (closes the
  first-input chicken-and-egg), emitting the slot-store suffix
  even when `body_has_return` is true (so the user's let RHS gets
  persisted), and a `mutable: bool` field on `VarSlot` so the
  synthetic slot-load `let` is correctly mutable for `var` bindings.
  Net effect: 5/5 of the originally-failing REPL parity fixtures
  (508/534/536/727/727b) are green. `REPL_KNOWN_SKIP` is back to
  the empty-baseline shape; the entries documenting the prior gate
  are gone from `tests/release-e2e/run.sh`.
- **`Int`/`Float` arithmetic mismatch now errors cleanly (E0707) instead
  of crashing the backend.** A mixed-numeric binary operator (e.g.
  `a - 3.5` with `a: Int`) previously slipped through type-checking with
  the left operand's type, then tripped the Cranelift verifier in codegen
  (`isub.i64 … arg has type f64`) / a JIT panic in the REPL. `typeck`
  now reports E0707 at the operator's source span; convert one operand
  (`a.to_f() - 3.5`) so both sides share a type. New registry entry +
  `docs/errors/E0707.md`.
- **Unknown method on a scalar primitive now errors at type-check
  instead of crashing the JIT.** `a.to_f()` / `a.bogus()` on an `Int`
  used to mangle to `Int_to_f` with no runtime symbol and panic the REPL
  (`can't resolve symbol Int_to_f`). `typeck` now emits a clean
  `no method \`X\` on type \`Int\`` for numeric / `Bool` / `Char`
  receivers, matching the existing field-access diagnostic so the
  brace-less (`a.to_f`) and braced (`a.to_f()`) forms fail identically.
  Class / struct / enum / collection / generic receivers keep their
  later-phase resolution path unchanged.
- **REPL: skip the tail-preservation transform when the wrapper body
  contains an embedded `return`.** The transform added in Task 1.3
  (the REPL state refactor) hoists the trailing expression into
  `let __ruxen_repl_tail_<fn> = <expr>` and re-emits the tail-name
  identifier as the new tail; the wrapper's return type infers from
  that. When the user's input embeds a `return` along a non-tail
  path — e.g. `if cond; puts ...; return; end` inside a top-level
  expression statement, or the same shape replayed from
  `session_var_mutations` into a later input's wrapper — the wrapper
  ends up with two exit points whose return types disagree, and
  Cranelift's verifier rejects it with `arguments of return must
  match function signature` (the failure on
  `727_async_tcp_echo`'s `if handle == 0; puts "spawn_fail"; return;
  end` REPL input). New `statements_contain_return` /
  `expr_contains_return` / `block_contains_return` helpers walk the
  user's statement list (and the replayed statement list) looking
  for an embedded `Return` AST node anywhere — recursing through
  every control-flow / call / binary / unary / block / cast / index
  / try / await / assign / range / array-literal / tuple-literal /
  map-literal / enum-variant / macro-call form, but stopping at
  closure bodies (a `return` inside a closure exits the closure, not
  the wrapper). When the walker reports any embedded return,
  `build_program` skips the tail-preservation rebind: the embedded
  `return` becomes the actual exit, and the wrapper's signature
  infers naturally from the remaining tail (or Unit when the tail
  is a control-flow block). Three new unit tests under
  `src/ruxen_repl/src/tests/return_in_block.rs` pin the contract
  (return-inside-if-then, return-inside-if-no-else,
  return-inside-else-arm). Phase 2 (slot-store re-ordering /
  wrapper-signature widening) is still needed for the full
  `727_async_tcp_echo` REPL parity run — a later input
  (`let ok = client_flow()`) trips the same verifier error on the
  *replayed* `return` from the earlier `if handle == 0; ...; return;
  end`, which this fix detects but cannot fully resolve without
  widening the wrapper signature to accept a bare `return`.
- **REPL: coerce wrapper return type to Unit when the body contains
  any `return` (user OR replayed).** Phase 1 (above) skipped the
  tail-preservation rebind when the wrapper body contained an
  embedded `return`, but skipping the rebind alone left the
  wrapper's natural tail in place — so on a later input like
  `let ok = client_flow()` the synthetic display tail
  (`Statement::Expression(Identifier(ok))` appended by
  `eval_statement`) made the wrapper infer return type `Int`, and
  the *replayed* bare `return` from the earlier `if handle == 0;
  ...; return; end` then re-tripped the verifier with `arguments
  of return must match function signature` — this time on
  `__repl_4` (`let ok = client_flow()`) in the 727 REPL parity
  run. `build_program` now, when `body_has_return` is true, strips
  a pure display-read tail (any `Statement::Expression` whose
  expression is a bare `Identifier(_)` — exclusively the synthetic
  read appended after a let so the new binding can be displayed)
  and appends an empty `Block` (which evaluates to Unit) as the
  new tail. Side-effecting expression statements like `puts
  "reached"` are kept as-is — they execute as intermediate
  statements; only the synthetic display name is dropped. The
  wrapper's signature unambiguously infers as Unit and both the
  bare `return` (lowered as `return_(&[])`) and the synthetic
  Unit tail satisfy it. User-visible: inputs that contain a
  `return` (directly or via replayed history) no longer display
  their natural tail value via `=> <value> : <ty>`. This matches
  compile-and-run semantics (`def main; …; return; end` returns
  Unit and has no display value). Two new tests under
  `src/ruxen_repl/src/tests/return_in_block.rs` pin the contract:
  `let_after_replayed_return_compiles` (let-binding follows a
  return-containing input) and `puts_after_replayed_return_compiles`
  (a `puts` side-effect statement follows the same). Phase 3 of
  the unblock plan will remove `727_async_tcp_echo` and `727b`
  from the `REPL_KNOWN_SKIP` gate once their full REPL parity is
  confirmed end-to-end (manual run after this patch shows no
  verifier error through the entire translated input; `echo_ok`
  surfacing is gated on a separate slot-widening question for
  Thread handles tracked as Phase 3+ follow-up).
- **REPL: filter slot-backed `let` bindings out of the replay stream
  + populate slot stores even when the wrapper body contains a
  `return`.** `collect_replay_statements` now drops
  `Statement::Let` entries whose pattern is a single-identifier
  name that's currently slot-backed, and also drops
  `Statement::Expression` entries that are bare `Assign` /
  `CompoundAssign` to the same names. The wrapper's synthetic
  slot-load prefix is the source of truth for those values;
  replaying the original let-RHS would re-execute side-effecting
  initializers (`Thread.spawn_raw`, network bind, file open) AND
  shadow the slot-loaded binding with a fresh (possibly wrong)
  value — and replaying assignments on top of an already-up-to-date
  slot load would double-count (`counter = counter + 1`). To make
  the slot the source of truth from the very FIRST input that
  registers a slot variable, `eval_statement` now runs a probe
  typecheck on a no-slot-ops wrapper to discover the binding's
  inferred type and pre-registers slot-eligible names BEFORE the
  real `build_program` call, so the slot prefix/suffix pair lands
  around the user's `let` and the suffix `__slot_store_i64` captures
  the freshly-bound value. The slot store suffix is now also
  emitted when `body_has_return` is true (without the tail-preserve
  rebind — Phase 2's Unit-coercion still strips the natural tail
  and appends `Block(())`); the replayed `return` typically doesn't
  fire (its condition reads a slot-loaded value whose current state
  makes the if-branch false), the user's let-RHS runs to
  completion, and the store persists the new value for the next
  input. `VarSlot` gains a `mutable` flag plumbed from
  `LetBinding.mutable` so `var foo = …` renders as a mutable
  slot-load in subsequent wrappers and user `foo = expr`
  assignments aren't rejected by E1006 ("cannot assign to `let`
  binding"). For `727_async_tcp_echo` specifically: `let handle =
  Thread.spawn_raw(...)` was being replayed on every subsequent
  input, the second pthread spawn's bind hit EADDRINUSE,
  `server_loop` returned 0, the lexical rebind made `handle = 0`,
  and the replayed `if handle == 0; puts "spawn_fail"; return;
  end` exited the wrapper before the user's actual input could
  run — that's the test_repl_cases hang. With the filter +
  pre-registration + body_has_return slot store, the slot load
  of `handle` (the original valid `pthread_t`) survives; replay
  no longer re-spawns; the user's later `let ok = client_flow()`
  populates slot[ok] correctly via the in-wrapper store; the if-
  else echo-print sequence reads the correct slot value; the
  test produces `echo_ok` exactly as compile-and-run does. Two
  new tests under `src/ruxen_repl/src/tests/return_in_block.rs`
  pin the contract (`slot_backed_let_does_not_shadow_via_replay`
  for the literal-RHS case, `slot_backed_let_with_call_rhs_does_not_replay`
  for the call-RHS shape that mirrors 727's `Thread.spawn_raw` /
  `client_flow` lets). Closes the 727 / 727b REPL parity hang —
  Phase 3 (removing the `REPL_KNOWN_SKIP` entries) can now
  proceed.
- **Restore `include Future` + `type Output = Result[...]` on 14 stdlib
  future classes accidentally stripped by Phase 3.** Commit `3dc02b6`
  (the runtime replay-suppression flag) modified 80+ stdlib `.rx`
  files as a side effect of an automated stylistic pass — including
  removing the `include Future` mixin inclusion and the
  `type Output = Result[..., ...]` associated-type declaration from
  every async future class (`async_open_future`,
  `async_read_to_string_future`, `async_write_all_future`,
  `async_read_line_future`, `async_accept_future`,
  `async_bind_future`, `async_close_future`, `async_connect_future`,
  `async_read_future`, `async_read_with_timeout_future`,
  `async_write_future`, `task_join_future`, `task_yield_future`,
  `time_sleep_future`). Without the trait inclusion, executor
  polling lost the Future contract and any fixture exercising
  block_on of an async path returned wrong results or segfaulted
  on type mismatch — `case/740_async_stdin_read_line_eof` returned
  `eof_fail` (the Future-side read couldn't be polled), and
  `case/727b_async_tcp_read_timeout` segfaulted on the executor's
  attempt to drive an unstamped AsyncTcpListener.bind. The 14
  affected files have been reverted to their `acde6476` shape;
  Phase 3's intended runtime + REPL changes (the `__thread int
  ruxen_repl_is_replaying` flag and the wrap layer in
  `library/std/*/runtime/*.c`) remain in place. Both fixtures now
  PASS under the compile path; the 5 REPL parity baseline
  (508/534/536/107/etc.) stays green.
- **REPL parity sweep: 727_async_tcp_echo + 727b_async_tcp_read_timeout
  gated via `REPL_KNOWN_SKIP`.** Both fixtures bridge a sync
  `Thread.spawn_raw` server with an async client over a hardcoded
  localhost port. Phase 3's replay-suppression flag intentionally
  leaves `Thread.spawn_raw` unwrapped (so 555 / 725 clock fixtures
  work), so REPL replay re-runs the spawn; the second bind hits
  `EADDRINUSE`, the server thread returns 0, and the fixture's
  spawn-fail guard exits before any output reaches stdout. Fixing
  properly requires either a persistent async executor whose
  listener state survives across REPL inputs, or a per-session-var
  binding for `Thread`/`JoinHandle`/`AsyncTcpListener` constructors
  so each runs exactly once. Both are multi-day refactors outside
  v1 release scope. The `REPL_KNOWN_SKIP` comment in
  `tests/release-e2e/run.sh` documents the pickup point.
- **`ruxen fmt`: re-synced the AST formatter with current parser
  semantics.** A corpus round-trip test (`tests/
  formatter_corpus_roundtrip.rs`, every `.rx` in `library/std` +
  `tests/release-e2e/cases`) found 129/386 files whose formatted output
  no longer parsed. Ten distinct drifts fixed in
  `compiler/ruxen_core/src/formatter/`: (1) `lib "runtime/foo.c"` lost
  its quotes (`format_lib_name` now re-quotes any non-TypeIdentifier
  link name); (2) struct/class fields gained a bogus `pub ` prefix
  (`pub` is not a keyword — `format_field_section` now emits
  `public`/`private`/`protected` section markers per ruby-naming.spec.md
  §3.2, default-public emitting nothing); (3) FFI `def self.NAME` lost
  its `self.` class-method marker; (4) mixin method signatures emitted
  `var def` instead of `def var`; (5) `extension` blocks were emitted as
  the retired `impl` keyword; (6) `const X = v` gained a spurious `: _`
  inferred-type annotation; (7) `do … end` block expressions dropped
  their `do`/`end` delimiters; (8) `move` / `async` closure modifiers
  were dropped; (9) the `Never` type was emitted as `!` (not a valid
  type token); (10) multi-statement `match` arms gained a spurious
  per-arm `end`. All 386 corpus files now round-trip and are
  idempotent; 11 targeted regression tests added in `formatter/tests.rs`.
- **Parser: accept `;` as a leading terminator in block bodies.**
  `parse_body_with_options` (the common loop behind every `def`/`if`/
  `while`/`for`/`match`-arm/class body) called `skip_newlines()` at the
  loop head, which doesn't consume `Semicolon`. As a result, single-line
  idioms like `def double(n: Int) -> Int; n * 2; end` and `def noop ->
  Unit; end` failed with a misleading `expected expression, found
  Semicolon`. Added `Parser::skip_terminators()` (consumes both
  `Newline` and `Semicolon`) and switched the body loop to use it.
  `expect_terminator` already accepted either form after a statement;
  this brings the loop head into line. No semantic change for
  multi-line bodies. Closes the REPL safety-net test
  `def_callable_from_later_input`.

### Added
- **REPL session-variable slot writeback (refactor phase 1.3 — Path C step 2).**
  `build_program` now appends a synthetic
  `__slot_store_i64(<addr>, <name>)` for every slot-eligible Int session
  variable, so a mutation in one REPL input (e.g. `x = x + 5`) is
  persisted to the variable's fixed slot and visible (via the load
  prefix from phase 1.2) on the next input. The wrapper's trailing
  tail expression is hoisted into a fresh local before the stores and
  re-emitted after, preserving the wrapper's return type so the
  `=> <value> : <ty>` display path is unchanged. Closes the slot
  read+write loop for primitive Int session vars; the
  `all_statements` replay still runs in lockstep — phase 3 drops the
  replay and the slot becomes the sole source of truth. Also fixes the
  `mutation_persists_across_inputs` REPL safety-net test that used the
  retired `let mut` syntax (now `var`).
- **REPL session-variable slots (refactor phase 1.2 — Path C step 1).**
  `ReplSession::register_var` is now wired on every successful `let`
  binding (`CompileHook::RecordLet`), allocating a persistent 8-byte
  slot per primitive Int variable and reusing it on rebind. `build_program`
  injects a REPL-internal `lib "ruxen_repl" ... end` block declaring
  `__slot_load_i64` / `__slot_store_i64` (aliased to the runtime symbols
  `ruxen_repl_slot_load_i64` / `ruxen_repl_slot_store_i64`) plus a
  synthetic `let <name>: Int = __slot_load_i64(<addr>)` prefix for every
  Int session variable, giving the typechecker an explicit binding from
  the slot. The `all_statements` replay still runs alongside (the
  replayed let shadows the synthetic prefix during phase 1.2); phase 3
  removes the replay once the store suffix lands in phase 1.3.
- **Test framework — `ruxen test` + pure-Ruxen `std.test` (T3.03).**
  A new `std-test` stdlib package ships an RSpec-style DSL —
  `Tester.describe`/`context`/`it`/`xit`/`before`/`after` with
  `t.expect(x).to_eq/.not_to_eq` plus `BoolMatcher`/`OptionMatcher`/
  `ArrayMatcher`/`StringMatcher` and `t.it_panics(name, substr)`. The
  `ruxen test` subcommand discovers `tests/**.rx` (skipping
  `tests/support/`), synthesises a per-file `def main` that drives a
  `Runner`, builds each file through the incremental cache, and fans the
  binaries out across worker threads — each `it` runs in a forked child
  so a panic isolates to one case. Flags: `FILTER`, `--release`,
  `--test-threads`, `--fail-fast`, `--nocapture`, `--list`, `--no-run`,
  `--include-pending`, `--format=pretty|tap|json`. `ruxen new` scaffolds
  a green `tests/example.rx`. v1 limitations (substring verification for
  `it_panics`, after-hook inheritance into nested `context`) are tracked
  in `docs/tutorial/19-writing-and-running-tests.md`.
- **VS Code extension is releasable (`editors/vscode` v0.1.0).** The
  extension now launches the language server via the unified `ruxen`
  binary's `lsp` subcommand (configurable through the new
  `ruxen.server.args` setting), ships `package`/`publish` npm scripts,
  marketplace metadata, a README, and a license pointer. Packaging no
  longer strips `node_modules`, so the `.vsix` bundles its
  `vscode-languageclient` runtime dependency and starts correctly.
- **Package manager — workspaces, `ruxen publish`, `ruxen update
  --precise`.** `Ruxen.toml` now accepts a `[workspace]` table with
  literal members and trailing-`*` globs; intra-workspace deps
  resolve by bare-name shorthand (`pkg-b = "0.1.0"` finds the
  sibling member), and every member shares `target/` at the
  workspace root. `ruxen publish [--dry-run] [--registry <name>]`
  packages the current package into `<name>-<version>.tar.gz` and
  pushes a `v<name>-<version>` git tag to the configured remote
  after probing for collisions. `ruxen update --precise <pkg>=<rev>`
  pins a single git dep in `Ruxen.lock` without re-resolving the
  rest. New diagnostic codes: E1600 (workspace member not found),
  E1601 (intra-workspace path-dep cycle), E1602 (publish tag
  already exists at remote).
- **#06.8 stdlib self-hosting — Waves 1.5–2 + collection migrations
  (T#13/14/15/16/17/21).** Every named stdlib module and every
  collection-method dispatch table that previously lived as Rust
  registrations in `compiler/ruxen_core/src/resolve/stdlib/mod.rs`
  and `codegen/runtime_table/mod.rs` now lives in `library/std/src/
  *.rx`. The bootstrap loader (`resolve/bootstrap.rs`) parses
  these files at compiler startup before user code, the resolver's
  **namespace-anchor mode** reuses the existing type-scope bindings
  for builtin names (so `Ty::String` / `Ty::Array(_)` etc. stay
  canonical for codegen), and MIR's **FFI alias map** rewrites the
  mangled call-site callee (`Array[Int]_push`, `Option[String]_unwrap_or`,
  ...) to the verbatim C symbol BEFORE codegen consults
  `runtime_table`. A generic-stripping fallback peels surface
  `[...]` args so the parent-name-keyed alias entries
  (`Array_push`, `Option_unwrap_or`) match every call site. 94
  collection methods migrated across String / Option / Result /
  Array / Map / Set; ~12 self-hosted .rx modules under
  `library/std/src/`. Two outliers retained in `runtime_table` for
  documented architectural reasons (`String_clone` aliases a C
  symbol with a different wire shape and trips E0722; `String_from_iter`
  has no surface method to attach to).
- **#06.5 Phase 2 sync I/O completeness.** TcpListener/TcpStream
  classes (incl. binary-safe read + socket timeouts), BufReader[R] /
  BufWriter[W] over the closed inner set {File, TcpStream}, std.rand
  (kernel CSPRNG), Command + Instant promotion to the canonical
  surface (legacy `process_run` / `now_ns` free-fns retired).
  Mixin/trait-druxen Read/Write surface is deferred to v1.5; closed-set
  inner check at typeck (E0714) keeps the runtime kind-tag honest.

### Fixed
- **compiler: `any Fn(...)` closures are now dispatchable via
  indirect call (#06.9).** Works in let-binding, array storage,
  class field, and return-position contexts. Unblocks Router /
  event-bus / observer-pattern code. Three sites changed
  (`typeck/unify.rs` lifts the closure → dyn-Fn coercion out of
  arg-only position; `mir/lower/expr/method_call.rs` recognises
  `Ty::AnyMixin([Fn])` receivers, falls back to the local's MIR
  type when typeck left the for-loop binding as `Ty::Infer`, and
  emits the same indirect-call shape the concrete-`Fn` path
  already uses; `codegen/runtime_table/mod.rs` adds a
  belt-and-suspenders fast-path for `any Fn[...]_call` and `?T*_call`
  manglings). 5 pin tests in
  `compiler/ruxen_core/tests/closures_dyn_dispatch.rs`; e2e
  fixture `tests/release-e2e/cases/600_closure_handler_dispatch.rx`.

### Changed
- **Repo restructured to rust-lang-style layout (#06.75).** The
  single `crates/ruxen-core` crate (≈26 KLOC, owning lexer / parser /
  hir / resolve / typeck / mir / borrow_check / codegen / formatter /
  the C runtime / every stdlib registration) is unpacked into a
  `compiler/` + `library/` + `src/` + `tests/` top-level tree:

  - `library/runtime/` — the C runtime, carved per module
    (`core/{alloc,vec,string,hash}.c`, `io/{io_error,stdio,file}.c`,
    `fs.c`, `net/tcp.c`, `time.c`, `signal.c`, `process.c`, `fmt.c`,
    `env.c`).  Top-level `library/runtime/runtime.c` `#include`s each
    module so the build product is still a single translation unit
    with identical link symbols.
  - `library/std/src/` — the `.rx`-source side of the stdlib
    (currently just `iter.rx` as declarative documentation).
  - `compiler/ruxen_core/` — the (still-single) compiler crate,
    relocated from `crates/`.  Internal stdlib registrations now live
    under `compiler/ruxen_core/src/resolve/stdlib/` (carved out of the
    7 153-LOC `resolve/mod.rs`), method resolvers under
    `compiler/ruxen_core/src/typeck/method_resolvers/` (carved out of
    `typeck/infer.rs`), and the runtime-name table under
    `compiler/ruxen_core/src/codegen/runtime_table/` (carved out of
    `codegen/runtime.rs`).
  - `compiler/ruxen_driver/` — a thin `pub use ruxen_core::*` shim,
    placeholder for the future per-phase crate split (`ruxen_lexer`,
    `ruxen_parser`, `ruxen_resolve`, `ruxen_typeck`, …) which is
    deferred to a follow-up prompt.
  - `src/` — every driver crate (`ruxenc`, `ruxen_cli`, `ruxen_lsp`,
    `ruxen_ide`, `ruxen_repl`), relocated from `crates/`.
    Kebab-case package names switched to snake_case to match their
    lib names — `use ruxen_core::…` import sites unchanged.
  - `tests/` — workspace-level integration root (currently houses
    `release-e2e/`; per-crate Cargo integration tests stay under
    `compiler/ruxen_core/tests/` since Cargo's integration-test
    convention is per-crate).

  Workspace `members` is now `["compiler/*", "src/*"]`.  C-side symbol
  names are byte-identical; no language surface changes; no behavior
  changes.  Architecture overview: `docs/architecture/repo-layout.md`.

### Added
- Phase 2 stdlib #06: `std::process::Command` builder API closes the
  last #06 v1 gap. `Command.new(prog).arg("a").args(["b","c"])
  .env("K","V").current_dir("/tmp").status()` returns
  `Result[ExitStatus, IoError]`; `.output()` returns
  `Result[Output, IoError]` with captured stdout/stderr. `Output`
  exposes `.status` / `.stdout` / `.stderr`; `ExitStatus` exposes
  `.code` / `.success`. Pre-flight `access(F_OK)` turns typo'd binary
  paths into `Result::Err(IoError::NotFound)` rather than the
  indistinguishable `Ok(ExitStatus(127))` that the lower-level
  `process_run` returns. Mirrors the `fs.metadata` flat-heap-struct
  pattern with one extension: `Command` is in `user_drop_classes` so
  builder-pattern temporaries are reclaimed via `Command_drop` +
  `ruxen_dealloc` at scope exit. 9 pin tests in
  `stdlib_process.rs::command_*`; e2e fixtures `508_command_status`
  and `512_command_output`. `Command.spawn -> Child` (the async-style
  handle with `.wait/.kill/.try_wait`) is explicitly DEFERRED to v2
  per the prompt.
- Phase 2 stdlib #06: `std::fs::metadata(path) -> Result[Metadata,
  IoError]` returning a flat heap-allocated `Metadata` struct with
  `len` / `modified` / `is_file` / `is_dir` / `is_symlink` accessors.
  Backed by `lstat(2)` so symlinks are reported as Symlink rather than
  followed. The on-wire layout (3 × int64: size / modified-secs /
  kind-tag) is packed by the runtime so the FFI surface is independent
  of libc's `struct stat`. Pin tests in `stdlib_fs.rs::fs_metadata_*`
  (positive file, positive dir, negative missing-path); e2e fixture
  `507_fs_metadata`.

### Added
- Phase 3 #07.S9 parser cut: `where`-clause const predicates.  The
  parser now accepts `where N > 0`, `where N == M`,
  `where N + M == 8` (and any mix of `> < >= <= == !=` comparisons +
  `+ - * /` arithmetic) on const-generic functions / types.  Each
  predicate is captured as a raw parser `Expr` on the new
  `WhereClause::const_predicates: Vec<ConstPredicate>` field;
  existing `WhereClause::predicates` (mixin bounds) is untouched, so
  existing consumers see no shape change.

  Disambiguation: a one-token lookahead past the leading identifier
  picks the path.  If the next token is a comparison or arithmetic
  op, parse as a const predicate; otherwise fall through to the
  historic mixin-bound parser (which expects `:`).  Mixed clauses
  (`where T: Display, N > 0`) split correctly into the two lists.

  Spec stage map S9 entry updated to "in flight" with the parser-
  cut commit; per-instantiation enforcement +
  `E-CONST-WHERE-FALSE` diagnostic remain pending the deeper S7
  binding-threading work.

  Pin tests in `const_generics.rs`:
  `parse_where_clause_const_predicate_n_gt_zero`,
  `parse_where_clause_const_predicate_n_eq_m`,
  `parse_where_clause_const_predicate_arithmetic_eq`
  (`N + M == 8` — verifies the LHS itself is an arithmetic tree),
  `parse_where_clause_mixed_trait_bound_and_const_predicate`
  (both forms in one clause split correctly),
  `parse_where_clause_trait_bound_alone_still_works` (regression
  gate for the historic shape).

### Added
- E2E harness supports a `RUXEN_E2E_CASES` env-var case filter for
  selective runs.  Comma-separated case stems (filenames without
  `.rx`); absent var preserves the existing behaviour (full
  sweep).  Whitespace is trimmed; empty entries ignored; unknown
  cases fail fast with a clear message so typos don't silently
  skip the case the developer was trying to verify.

  New workflow:
  - **Per-commit**: skip e2e entirely (`#[ignore]`-gated as before).
  - **New / changed fixture**:
    `RUXEN_E2E_CASES=NAME cargo test --test release_e2e_smoke
     -- --ignored` runs just that one in ~1s.
  - **Phase / tier completion**: `cargo test --test
     release_e2e_smoke -- --ignored` runs the full ~3-min sweep.

  Documented inline at `tests/release_e2e_smoke.rs::release_e2e_
  all_fixtures`.

### Fixed
- **Const-generic class instantiation parses correctly at value-
  expression call sites.**  Before this fix, `Counter[10].new(42)`
  silently misparsed as `Counter` indexed by `10` followed by
  `.new(42)`, producing an `<error>` receiver type and a downstream
  linker failure (`Undefined symbols: _<error>_init`).  Root cause
  was the `looks_like_type_args` lookahead in `parser/expr.rs`
  rejecting `IntLiteral` as a potential first token after `[` — it
  only accepted type-token kinds (`TypeIdentifier`, `SelfType`,
  `Lifetime`, `Amp`, `AmpMut`).  Adding `TokenKind::IntLiteral(_, _)`
  to the accepted set lets `Counter[10].new(42)` correctly parse as
  a const-generic type application followed by a method call.

  Pin test:
  `parse_const_arg_int_literal_at_call_site_is_type_application`
  asserts the receiver of `.new(42)` is a `MethodCall`, NOT an
  `Index` expression (the regression shape).

  E2E fixture:
  `tests/release-e2e/cases/073_const_generic_class_instantiation.rx`
  exercises `Counter[10].new(1)`, `Counter[1000].new(2)`, and
  `Pair[Int, 7].new(99)` through full compile + run, confirming
  multi-instantiation and mixed type/const generics all work.
  Release-e2e harness now runs 223 fixtures (was 222).

  Spec stage map S6 entry updated: runtime construction of
  const-generic classes whose body doesn't reference the const
  param is now shipped; per-instantiation MIR lowering for bodies
  that *do* use the const param (e.g. `class Buf[const N: USize] {
   data: [Int; N] }`) remains a v1.next follow-up tracked in the
  S7 follow-up section.

### Added
- `tests/release-e2e/cases/072_const_generic_array_arithmetic.rx`:
  new end-to-end fixture exercising the S8.S2/S8.S4 array-size
  arithmetic path through full compile-and-run.  Covers literal
  arithmetic (`2 + 1`), paren-grouped precedence
  (`(1 + 1) * 2`), and identity folding (`5 + 0`).  Verifies that
  the const evaluator + normal-form rewriter agree with the
  codegen layout pass — three arrays of sizes 3 / 4 / 5
  initialise correctly and read back the expected elements.

  The release-e2e harness (`release_e2e_all_fixtures`) now runs 222
  fixtures (was 221; +1 const-generic case).  Note: const-generic
  *class* / *struct* instantiation through codegen
  (`Counter[10].new(...)`) still hits the per-key MIR lowering
  gap tracked as the S6 follow-up; that's a separate workstream
  from the array-size form which this fixture pins.

### Changed
- `docs/tutorial/21-const-generics.md` rewritten end-to-end to
  match the shipped reality.  Previously the status block said
  "Stage 1+2 shipped, 3–9 pending" and the body taught only literal
  use sites.  Updated to cover: bare literals, `+ - * /`
  arithmetic in both array-size and const-arg positions, the
  normal-form rewriter (`2 + 2`, `4 * 1`, `4 + 0` all unify with
  the bare `4` form), the four shipped diagnostics
  (E0702 / E0703 / E0704 / E0705) with cross-links to their
  detailed pages, distinct-types semantics, S5 soundness fix,
  and the two genuinely-remaining v1 follow-ups (S7
  binding-threading, S9 where-clause predicates).  Stage table
  removed in favour of a "what's pending" callout — the spec is
  the canonical stage map.  Examples chosen to mirror the
  pin-test bodies in `tests/const_generics.rs`.

### Added
- Phase 3 #07 follow-up: **E0702 non-const-expression diagnostic**
  (spec §B8 E-CONST-NONCONST).  The lowerer
  `lower_const_expr_from_expr` already produced a
  `ConstExpr::Error` marker for AST shapes outside the v1 const
  language (unsupported binary ops like `%` / `<` / `<<` / `&&`,
  function calls, method calls, field access, runtime variable
  references); resolve now walks the lowered tree and surfaces
  the first `Error` it finds as **E0702** at the construction
  site's span.  One diagnostic per site — nested markers don't
  compound.

  Helpers:
  - New `contains_const_expr_error(&ConstExpr) -> bool` walks the
    tree.
  - New `Resolver::check_const_expr_for_non_const(&mut, &Span)`
    emits the diagnostic; called at the same two sites as
    `check_const_expr_eval_errors` (array size + const-arg
    position).

  Registry / docs / spec coordination:
  - `codes::REGISTRY` title for E0702 dropped its "(reserved)"
    qualifier and now reads "expression is not a valid v1 const
    expression".
  - `docs/errors/E0702.md` rewritten with concrete examples
    (`%`, comparison, function call), a fix matrix (rewrite, lift
    to const param, declare a `const`), and notes referencing
    NG-OQ-3 (no const-arg inference).
  - Spec §"Error code reservations" updated — E0702 marked
    shipped; E0701 remains the lone reserved code.

  Pin tests in `const_generics.rs`:
  `array_size_unsupported_op_emits_e0702` (`5 % 2`),
  `array_size_comparison_op_emits_e0702` (`3 < 4`),
  `array_size_clean_arithmetic_does_not_emit_e0702`,
  `array_size_param_reference_does_not_emit_e0702`.  The existing
  `resolve_array_size_unsupported_op_becomes_error` (HIR-shape
  pin) continues to pass — E0702 is additive, not a replacement.
- Phase 3 #07 follow-up: **E0705 const-generic parameter bad-type
  diagnostic** (spec §B8 `E-CONST-BAD-TYPE`).  `Resolver::
  collect_generic_params` now validates the resolved type of every
  `const NAME: TY` parameter against the v1 allow-list (integer
  family + `Bool`).  Float* / String / user class / Array / tuple /
  every other shape surfaces as **E0705** with the parameter name
  and the rejected type in the message.  `Ty::Error` is treated as
  valid so the diagnostic doesn't stack on top of an upstream
  "unknown type" error.

  New helper `is_valid_const_param_ty(&Ty)` placed alongside the
  existing `ty_is_valid_hash_key` in `resolve/mod.rs`.  Registry
  entry, `EXPLAINS` row, and `docs/errors/E0705.md` page added per
  the existing three-way sync invariant; the page documents the
  full rejected-types list with NG2 / NG3 / OQ-3 cross-references.

  Pin tests in `const_generics.rs`:
  - `const_param_float_type_emits_e0705` — NG2 (NaN ≠ NaN).
  - `const_param_string_type_emits_e0705` — NG3.
  - `const_param_user_class_type_emits_e0705` — user aggregate
    types.
  - `const_param_integer_types_do_not_emit_e0705` — pins every
    integer family + `Bool` as accepted.
  - `const_param_bad_type_does_not_stack_on_unresolved_type` —
    `Bogus` (unresolved) doesn't compound diagnostics.

  Spec §"Error code reservations" updated to mark E0703/E0704/E0705
  shipped (E0701/E0702 remain reserved pending the const-arg-vs-
  param-type and non-const-expr emit sites).
- Phase 3 #07 follow-up: **E0703 surfacing for pure-literal const
  overflow / division-by-zero**.  New
  `Resolver::check_const_expr_eval_errors` runs immediately after
  the S8.S4 normal-form pass at every `TypeExpr::Array { size }`
  and `TypeExpr::ConstExprArg` resolve site.  It calls
  `eval(empty)` on the normalised tree and maps
  `ConstEvalError::Overflow` and `ConstEvalError::DivisionByZero`
  to **E0703** with the source span of the const expression.

  Pure-literal sub-trees whose eval failed survive the normaliser
  as `Op(Lit, _, Lit)` (the successful pure-literal `Op` cases
  collapse to `Lit`), so detection is local and accurate.  Trees
  that mention a const-generic parameter return
  `Err(Unresolved(name))` from eval and are silently skipped —
  their overflow status depends on the instantiation, so the
  check defers to the (still-pending) monomorphization-side pass
  that needs the per-instantiation binding-threading prep work.

  The registry / explain / docs trio refreshed:
  - `codes::REGISTRY` title for E0703 dropped its "(reserved)"
    qualifier and now reads "const expression overflows or
    divides by zero during evaluation".
  - `docs/errors/E0703.md` rewritten with concrete examples for
    `+` overflow, `/0`, nested multiplication overflow, and a
    note on why param-bearing trees aren't flagged.

  Pin tests in `const_generics.rs`:
  `array_size_overflow_emits_e0703` (`i64::MAX * 4` overflows
  u64), `array_size_division_by_zero_emits_e0703`,
  `const_arg_position_overflow_emits_e0703`,
  `array_size_param_arithmetic_does_not_emit_e0703`
  (`N + 1` deferred), `array_size_bare_literal_does_not_emit_e0703`,
  `array_size_clean_arithmetic_does_not_emit_e0703`.
- Phase 3 #07.S8.S4: `ConstExpr::normal_form()` rewriter canonicalises
  arithmetic trees so two source-level forms that denote the same
  compile-time integer produce the same `Ty::ConstArg`.  Rules
  applied bottom-up: pure `Lit ⊙ Lit` constant-folds via the S8.S1
  evaluator (overflow / div-zero leave the `Op` shape intact so a
  later E0703 surfacing pass keeps both spans); identity rewrites
  collapse `x + 0`, `0 + x`, `x - 0`, `x * 1`, `1 * x`, `x / 1`
  to `x`; `x * 0` and `0 * x` to `0`.  Applied at the resolve
  construction sites for both `TypeExpr::Array { size }` and
  `TypeExpr::ConstExprArg`, so `[T; N + 0]` and `[T; N]` compare
  equal through derived `PartialEq`.  Not handled (spec §B8
  intentional limit): distributive rewrites (`N*(M+1)` vs.
  `N*M + N`), commutative reordering of mixed `Param`/`Lit`,
  associative reassociation — v2 will surface
  `E-CONST-NORMAL-FORM` at the kind-check when two instantiations
  differ only by a form the rewriter can't canonicalise.

  Pin tests in `const_generics.rs`:
  `const_expr_normal_form_identity_rewrites` (all eight rules),
  `const_expr_normal_form_folds_pure_arithmetic` (basic + nested),
  `const_expr_normal_form_preserves_op_on_overflow` (`u64::MAX + 1`
  and `7 / 0` keep the `Op` shape),
  `const_expr_normal_form_recurses_into_children` (`(N + 0) * 1 = N`),
  `resolve_normalises_array_size_n_plus_zero_equals_n` (full
  parse-→-resolve round trip pinning `[T; N + 0] == [T; N]`),
  `resolve_normalises_const_arg_arithmetic_with_one_factor`
  (`Vector[Int, 4 * 1] == Vector[Int, 4]`).

  Four pre-existing S8.S2 tests (`resolve_array_size_lowers_*`)
  were rewritten to pin the post-fold `ConstExpr::Lit(n)` shape
  rather than the unfolded `Op` shape — eval round-trip
  assertions retained so any future representation change is
  still caught.  Spec stage map updated to mark S8 shipped;
  remaining tier-2 work is the S9 `where`-clause const
  predicates.
- Phase 3 #07.S8.S3: arithmetic in const-arg position
  (`Vector[Int, 2 + 3]`).  Parser's `parse_generic_arg` looks one
  token ahead after an `IntLiteral`; if a binary arithmetic op
  (`+ - * /`) follows, the whole expression is captured via
  `parse_expression` and emitted as the new
  `TypeExpr::ConstExprArg { expr: Box<Expr>, span }` AST variant.
  Bare literals (`Vector[Int, 4]`) continue to emit `ConstLit`
  unchanged — backwards-compat fast path preserved.  Resolve folds
  `ConstExprArg` through the existing
  `lower_const_expr_from_expr` helper that S8.S2 added for
  `[T; expr]` array sizes, so const-arg arithmetic produces
  identically-shaped `Ty::ConstArg(ConstExpr::Op(...))` HIR.  The
  kind-check now treats both `ConstLit` and `ConstExprArg` as
  const-kind args; a kind mismatch (const arithmetic landing in a
  type slot) still emits E0704 with the diagnostic message
  refined to say "found const expression" rather than "found const
  literal".  Pin tests in `const_generics.rs`:
  `parse_const_arg_arithmetic_emits_const_expr_arg`,
  `parse_const_arg_bare_literal_still_emits_const_lit`,
  `resolve_const_arg_arithmetic_lowers_to_const_expr_op` (full
  parse→resolve→typeck round trip through a function parameter
  annotation), and
  `const_arg_arithmetic_against_type_param_emits_e0704`.  Spec
  stage map updated to reflect S8.S3 shipped; S8 still in flight
  pending the normal-form rewriter (`[T; N + 0] = [T; N]`).
- Three formatter / printer arms also extended so the new
  `ConstExprArg` variant doesn't crash exhaustive matches in
  `parser/printer.rs`, `formatter/comments.rs`, and
  `formatter/format_type.rs`.  The Doc-native printer routes
  through `format_expr_short` for now — a custom Doc layout for
  const-arg arithmetic can come when formatting that surface gets
  style attention.

### Changed
- **Breaking diagnostic-code rename:** const-generic kind-mismatch
  diagnostic moved from **E0700** to **E0704**.  E0700 was originally
  given to this slot by `docs/specs/types/const-generics.spec.md`
  §"Error code reservations", but the typeck iterator-`sum`
  validator already emits E0700 with the "requires `Add`" framing
  — the codes were colliding.  Spec amended to use E0704;
  iterator-`sum` keeps E0700.  Affects:
  - `crates/ruxen-core/src/resolve/mod.rs`: kind-mismatch emit site
    now writes `"E0704"`.
  - `crates/ruxen-core/src/diagnostics/codes.rs`: new `CodeInfo`
    for E0704 ("kind mismatch on const-generic argument") with a
    header comment recording the collision resolution.
  - `crates/ruxen-cli/src/explain.rs`: new `include_str!` row so
    `ruxen explain E0704` works.
  - `docs/errors/E0704.md`: full Why / Example / Fix / Notes /
    Related stub with historical note.
  - Pin test renamed:
    `const_lit_against_type_param_emits_e0700` →
    `const_lit_against_type_param_emits_e0704`.

### Added
- Tier-5 / T5.04 phase 1 follow-up: register reserved const-generic
  diagnostic codes **E0701** (wrong const-arg type), **E0702**
  (non-const expression in const-arg position), and **E0703**
  (const-arg expression overflows during evaluation) per
  `docs/specs/types/const-generics.spec.md` §"Error code
  reservations".  Each gets a `CodeInfo` entry in
  `codes::REGISTRY`, an `include_str!` row in
  `ruxen-cli/src/explain.rs::EXPLAINS`, and a
  `docs/errors/<code>.md` page describing the (planned) trigger,
  example, fix, and relationship to sibling codes.  No emit site
  yet — the reservations document where future S8 / S9 work will
  surface eval errors (overflow / div-zero — see the §B8 mapping
  noted in `E0703.md`) and where the S5 kind-mismatch path will
  split into separate kind vs. type diagnostics.

  Also adds two reciprocal pin tests to
  `tests/error_code_registry.rs` so the registry, the
  `EXPLAINS` table, and `docs/errors/` stay in three-way sync:
  `every_registered_error_code_has_a_docs_page` (no registry
  entry without a docs page) and
  `every_docs_error_page_has_a_registry_entry` (no orphan
  docs page without a registry entry).  Catches the drift
  pattern that left E0700 with two simultaneous meanings (an
  open follow-up noted in the registry header).

  Stale `codegen/layout.rs::layout_of` comment updated to reflect
  S8.S1 evaluator semantics — `Op` arithmetic now folds via
  checked `u64` ops, and overflow / div-zero fold to size 0 here
  pending the monomorphization-side wiring that will surface them
  as E0703.
- Phase 3 stdlib remainder closure: docs/specs/stdlib/hash.spec.md
  backfilled.  Spec covers the `Hashable` mixin surface, the
  auto-synth path for `Hashable`, the `T: Hashable` generic bound
  dispatch (working today for user types), the `Map` /
  `Set` key-validity gate (`ty_is_valid_hash_key`), and the
  primitive runtime hashing (`ruxen_hash_bits`, `ruxen_hash_str`).
  Documents the v2 gap on user-callable `.hash_code` for primitives
  and on `T: Hashable` monomorphisation for primitive `T` (link
  fails today with `_T: Hashable_hash_code`); a new `#[ignore]`
  pin test in `implicit_mixin_dispatch.rs::primitive_int_and_string_
  dispatch_through_hashable_bound` documents the gap so future v2
  work can flip the `#[ignore]` off.

  Phase 3 stdlib task 236fa21f's other listed items are deferred to
  v2 per their canonical specs: `fs::metadata` (needs a struct
  surface, `fs.spec.md` §"Out of scope"), `env::set_var` (write-side
  helpers, `env.spec.md` §"Out of scope"), and the full
  `Command` builder (`process.spec.md` §"Out of scope").  v1 ships
  `process_run` for one-shot invocations.
- Phase 3 #07.S8.S2: source-level arithmetic in `[T; <expr>]`
  array-size positions.  The parser already accepted any expression
  in that slot via `parse_expression`; resolve now folds
  `BinaryOp { op: Add | Sub | Mul | Div, ... }` recursively into
  `ConstExpr::Op` trees rather than collapsing them to
  `ConstExpr::Error`.  `Identifier(name)` legs preserve as
  `ConstExpr::Param` (S3 wiring) so `[T; N + 1]` works inside a
  `const N: USize` parameter scope.  Other binary ops (`%`,
  comparisons, `&&`, bit / shift ops) still fall through to
  `ConstExpr::Error`, surfacing as `ConstEvalError::Malformed` if
  evaluated — the spec reserves `+ - * /` for v1 const generics.
  Pin tests in `const_generics.rs`: parse/resolve of `[Int; 2+3]`,
  every operator individually with eval round-trip, paren grouping
  vs. operator precedence (`(2+3)*4 = 20` vs `2 + 3*4 = 14`), const
  param reference inside arithmetic (`[T; N + 1]`), and the `%`
  fallback path.  Const-arg position (`Vector[Int, 2 + 3]`) is the
  S8.S3 follow-up — `parse_generic_arg` still only accepts a bare
  integer literal.
- Phase 3 #07.S8.S1: `ConstExpr::eval` now implements the `Op` branch
  for `+ - * /` against `u64` bindings.  Inner sub-trees are recursed
  on first, so unresolved-param / parser-recovery errors propagate
  unchanged.  Checked arithmetic on `+ - *` surfaces wrap-around as
  `ConstEvalError::Overflow`; `u64` borrow on `Sub` (`0 - 1`) also
  surfaces as `Overflow` — the spec reserves a single
  `E-CONST-OVERFLOW` slot.  `_ / 0` surfaces as
  `ConstEvalError::DivisionByZero` (E-CONST-DIV-ZERO).  Pin tests in
  `const_generics.rs` cover basic add/sub/mul/div, mixed
  param-on-left, nested precedence (`(2+3)*4` and right-grouped
  `2+(3*4)`), every overflow direction, both error propagations,
  and `[Int; 2+2]` round-tripping through `layout_of`.  Parser
  acceptance of `+ - * /` in const-arg position is the S8.S2
  follow-up.
- Phase 2 #06.D4: `FormatSpec` (width / align / fill / precision) is
  applied at runtime.  `lower_interpolation` emits
  `Formatter_new_with_spec(w, p, a, f)` when the lex-captured spec is
  non-default; `Formatter_buffer` pads with the requested fill char /
  alignment at finalize.  Float precision routes through
  `Float_to_string_prec` (snprintf `%.*f`); String precision routes
  through `String_truncate_chars` (UTF-8 char-count truncate); Int /
  Bool / Char ignore precision per Rust semantics.  Strings with a
  non-default spec are no longer short-circuited by the legacy
  string-like pass-through — they fall through to `String_fmt` so
  width / precision / align / fill all apply.  Covered by 7 new
  `stdlib_fmt_runtime.rs` tests and E2E fixture
  `tests/release-e2e/cases/071_interp_format_specs.rx`.  Out of
  scope (deferred to v2): width on `"#{x:?}"` debug-spec
  interpolation (debug path still bypasses the Formatter); sign /
  `#` alternate / `0` zero-pad / radix flags.
- Phase 2 #06.D2.S4: end-to-end fixture
  `tests/release-e2e/cases/070_interp_display_dispatch.rx` proves the
  Display dispatch path runs at runtime — a `class Money` whose body
  `include Display`s and supplies `def fmt` interpolates via `"#{m}"`,
  while inner `"#{self.cents}"` routes through synth `Int_fmt`.
  Mirrored as an inline cargo test
  (`stdlib_fmt_runtime.rs::interpolation_user_impl_display_money_round_trips`)
  so the default `cargo test --workspace` run exercises it without
  waiting for the `--ignored` `release_e2e_all_fixtures` harness.
- Phase 2 #06.D2.S3: route string interpolation through `Display::fmt`
  dispatch — `Formatter_new` → `{T}_fmt(value, fmt)` → `Formatter_buffer`
  for primitives (Char / Int / Float / Bool) and any type whose body
  `include Display`s with a user `def fmt`. Output is byte-identical
  to the legacy `ruxen_*_to_string` path because the Stage 1 synth
  fns wrap the same helpers. Auto-`Debug`-only types still fall back
  to `{Name}_to_debug` until users provide their own `Display`
  implementation. Closes prompt 06's "string interpolation routes
  through Display::fmt" DoD bullet.

  Also lands a small Cranelift-codegen fix: `coerce_call_args` now
  consults a `user_fn_param_tys` side-table (populated during Pass 0/1)
  when computing the call-site argument coercion. Without this, a narrow
  `Bool`(i8) argument to the new synth `Bool_fmt` would be unconditionally
  widened to i64 by the default fallback rule and fail Cranelift IR
  verification. Runtime helpers still flow through `runtime_signature`.
- Phase 2 #06.D2.S2: add `user_has_impl_display(ty) -> Option<String>`
  helper on the MIR lowerer for the Stage-3 interpolation rewrite.
  Walks the HIR program's `impl_blocks` (via the `trait_impls` map
  collected at lowering start; both are the Rust-side internal name
  for the user-visible `include` directive's record) and resolves
  through Ref / RefMut / Alias / Newtype. No call site uses the
  helper yet — pure plumbing.

  Also bundles four S1 review follow-ups: harden seven `functions[0]`
  index lookups in `mir/tests.rs` to `.find(|f| f.name == ...)`;
  strengthen `synth_primitive_fmt_functions_emitted` to assert
  `params.len() == 2` + `return_ty == Ty::Unit` + non-empty body;
  document "why unconditional emission" + transparent String pass
  in `mir/lower.rs`.
- Phase 2 #06.D2.S1: synthesize primitive `Display::fmt` MIR
  functions — `Char_fmt` / `Int_fmt` / `Float_fmt` / `Bool_fmt` /
  `String_fmt` are emitted at program lowering and wrap the existing
  `ruxen_<prim>_to_string` helpers via `Formatter_write_str`. No
  interpolation call site is rewritten yet (Stage 3 will switch
  `lower_interpolation`). Bundles three S0 follow-ups: leak-tracker
  visibility for `ruxen_fmt_formatter_free`; isolated pin tests for
  `Formatter.write_char` (ASCII) + `Formatter.len`; phase-designator
  alignment in the `write_char` placeholder.
- Phase 2 #06.D2.S0: land the `ruxen_fmt_formatter_*` C runtime
  implementations (`new` / `free` / `write_str` / `write_char` /
  `buffer` / `len`) that Phase A's CHANGELOG referenced but never
  committed. `_free` uses `_ORIG_FREE` sentinel + asm-label rebind
  (same pattern as `ruxen_string_free` / `ruxen_vec_free`); `_buffer`
  transfers buffer ownership and self-frees the Formatter struct.
  Also adds `"Formatter"` to the MIR lowerer's built-in constructor
  special-case list so `Formatter.new()` emits `Formatter_new` rather
  than `Alloc + Formatter_init`. Pin test: `stdlib_fmt_runtime.rs` —
  round-trips `Formatter.new()` + `.write_str` + `.buffer` end-to-end.
- Phase 2 stdlib `std::fmt` foundation surface (#06 fmt MVP, plan
  in `docs/superpowers/plans/2026-05-10-stdlib-fmt.md`):
  - **Phase A** — Display/Debug formal mixins registered with a
    reading `def fmt(f: &mut Formatter) -> Result[(), FmtError]`
    signature. `Formatter` and `FmtError` registered as built-in
    classes. Runtime: `RuxenFormatter { char* buf; size_t len, cap }`
    plus `ruxen_fmt_formatter_{new,free,write_str,write_char,buffer,len}`
    helpers; v1 always returns `Ok(0)` from write_*. A user
    `class T ... include Display ... def fmt ... end ... end`
    parses and typechecks.
  - **Phase B** — Format-spec lexing for `"#{x:spec}"`. The lexer
    captures `[fill align] [width] ['.' precision] ['?']` into a
    `FormatSpec` struct on `StringPart::Expr`, threaded through HIR
    `HirInterpolationPart::Expr { expr, spec }` and into MIR.
    Examples: `"#{x:?}"`, `"#{x:>10}"`, `"#{pi:.2}"`,
    `"#{x:*<10.3?}"`. Five lexer pin-tests; printer + formatter
    round-trip non-default specs.
  - **Phase C+D MVP** — `:?` typechecks for auto-synth `Debug` types
    and width/precision/align specs typecheck on numeric types; existing
    `_to_debug` synthesis on auto-Debug structs already produces
    the expected output. Full `lower_interpolation` refactor through
    `Display::fmt` (Phase D2) deferred to a follow-up session.
- Phase 2 stdlib `Iterator` typing for closure-based `*Iter.map` /
  `*Iter.filter` chains (#05 follow-up). Typeck now seeds closure
  parameters from iterator `Item` for `*Iter.map` and `*Iter.filter`,
  and `*Iter.map` rewrites the iterator item type to the closure-body
  result so downstream `.collect_vec` and `Array` methods see the
  mapped element type instead of the source one. New focused unit
  coverage in `crates/ruxen-core/tests/stdlib_iterator.rs` pins
  `v.iter.filter { ... }.count` and cross-type
  `v.iter.map { |n| "#{n}" }.collect_vec.join(",")`.
- Phase 2 stdlib `std::io` no-Result print conveniences (#06.1 partial):
  `Stdout.print(s)`, `Stdout.println(s)`, `Stderr.eprint(s)`, and
  `Stderr.eprintln(s)`. The `*ln` variants emit a literal `\n` after the
  user's text. Failures are silently swallowed — Rust-style
  panic-on-broken-pipe is a v1 simplification omission, the
  matching `write_str` / `flush` on the same handles still surface the
  IoError when explicit handling is desired. Four new C runtime fns
  (`ruxen_stdout_print`, `ruxen_stdout_println`, `ruxen_stderr_eprint`,
  `ruxen_stderr_eprintln`) wired through `codegen/runtime.rs` plus
  matching method-type entries in `typeck/infer.rs`. New integration
  tests in `crates/ruxen-core/tests/stdlib_io.rs` (4 tests) pin the
  per-stream stdout / stderr routing and the with/without-newline
  contract. `IoError` enum migration (the second half of prompt 06.1)
  remains deferred — it is a runtime layout change touching every
  Result-returning fn.
- Phase 2 stdlib `std::env` / `std::fs` additions (#06 partial): `env.vars()`
  snapshots the process environment into `Map[String, String]` (walks
  `extern char **environ`, splits at first `=`, heap-copies both halves
  via `ruxen_string_from`); `env.current_dir()` returns
  `Result[String, IoError]` via `getcwd` with a growing buffer;
  `fs.is_file(path)` and `fs.is_dir(path)` consult `stat()` and return
  `Bool` (matching `fs.exists`'s "false on error" convention so they slot
  into `if` predicates without `?`); `fs.read_dir(path)` returns
  `Result[Array[String], IoError]` of the directory entry names, skipping
  `.` and `..`. Five new C runtime fns (`ruxen_env_vars`,
  `ruxen_env_current_dir`, `ruxen_fs_is_file`, `ruxen_fs_is_dir`,
  `ruxen_fs_read_dir`) wired through `codegen/runtime.rs` and registered
  in the `std::env` / `std::fs` builtin modules. New integration tests in
  `crates/ruxen-core/tests/stdlib_env.rs` (3 tests) and
  `crates/ruxen-core/tests/stdlib_fs.rs` (4 tests) compile inline Ruxen
  programs and run them against staged temp dirs / sentinel env vars.
  Outstanding from prompt 06: `env.vars` value extraction via
  `Option[&String].get` interpolates the raw pointer (pre-existing v1
  limitation — slated for the `Display` interpolation refactor in 06.2);
  `fs.metadata` deferred (needs a struct surface).
- Phase 2 stdlib `Map[K, V]` Entry API: `m.entry(K).or_insert(V)` and
  `m.entry(K).or_insert_with { || V }` (#04 final batch). The chain is
  detected and inlined as a single MIR unit — there is no real
  `Entry[K, V]` runtime value, sidestepping the pointer-returning
  two-variant dispatch the prompt-04 deferred note flagged. Lowering
  emits `if !ruxen_hash_contains_key(m, k) { ruxen_hash_insert(m, k, v); }`
  so the lazy-default contract of `or_insert_with` is honoured: the
  closure body only runs on the missing-key path. The chain returns
  `Unit` (v1 simplification — Rust's `&mut V` return is deferred). Typeck
  rejects splitting the chain across statements (`let e = m.entry(k); e.or_insert(v)`)
  with a clear error so users do not silently fall through the lenient
  unknown-method path. New release-e2e fixtures
  `510_map_entry_or_insert.rx` and `511_map_entry_or_insert_with.rx`
  cover the populated, empty, and lazy-default paths; positive
  type-check tests in `crates/ruxen-core/tests/stdlib_map.rs` and
  matching negative tests in `stdlib_map_negatives.rs`.
- Phase 2 stdlib `Iterator` eager terminators on `*Iter` classes
  (#05 batch 1). `vec.iter.sum` and `vec.iter.count` now type-check
  and dispatch to the existing `ruxen_vec_sum` / `ruxen_vec_count`
  runtime helpers — the type-checker previously knew only
  `*Iter.filter` / `*Iter.map`, so any other terminator on an iter
  receiver was rejected by typeck before reaching codegen. New entries
  in `typeck/infer.rs` cover `sum` (returns the iter element type, so
  `Array[Int].iter.sum -> Int` and a future `Array[UInt64].iter.sum -> UInt64`
  flows the right type through use-sites) and `count` (returns
  `USize`, mirroring `Array.count`). Both helpers were already wired
  through `codegen/runtime.rs` for `VecIter` / `VecIntoIter` /
  `SplitIter`, so this is pure typeck plumbing — no new runtime fns,
  no new error codes. New release-e2e fixtures
  `tests/release-e2e/cases/601_iter_sum.rx` and
  `602_iter_count.rx` exercise both the populated and empty-iter
  paths; the empty case verifies the additive-identity surprise
  check from the prompt brief (sum of empty = 0).
  The remaining Iterator surface (`fold`, `all`, `any`, `take(n)`,
  `skip(n)`, `chain`, `zip`, `enumerate`, `collect[FromIterator]`,
  full `mixin Iterator` in stdlib source) is deferred to a follow-up:
  closing it requires either per-method MIR inliners (for the
  closure-takers, mirroring the `inline_each` / `inline_filter`
  template) or new `*Iter`-specific runtime helpers — verifying
  either path requires a 140 s round-trip through the
  `cargo test --test p05_e2e_check` e2e probe (the agent sandbox
  forbids invoking `target/release/ruxenc` directly), which is too
  slow to land the full surface in a single batch.
- Phase 2 stdlib `Iterator` closure terminators + lazy combinators
  on `*Iter` classes (#05 batch 2). Six new methods land:
  closure-taking eager terminators `fold(init) { |acc, item| body }`
  / `all { |item| pred }` / `any { |item| pred }`, and the
  non-closure lazy combinators `take(n: Int)` / `skip(n: Int)` /
  (already-passthrough) `enumerate`. The closure terminators inline
  at MIR via three new helpers in `mir/lower.rs`:
  `inline_fold` (Section 3.7 — seeds an accumulator local from the
  init expression, walks the vec with `ruxen_vec_len` /
  `ruxen_vec_get`, and reassigns the closure's return value back to
  `acc` each step), and `inline_all_any` (Section 3.8 — seeds the
  result with the vacuous answer for the operator and short-circuits
  on the first counter-example, mirroring Rust's `Iterator::all` /
  `::any` semantics — empty iter → `all=true`, `any=false`). The
  lazy combinators `take` / `skip` eager-materialise into a fresh
  `RuxenVec*` via two new C runtime fns `ruxen_vec_take` /
  `ruxen_vec_skip` in `crates/ruxen-core/runtime/runtime.c`
  (clamped n; shallow element copy, matching `ruxen_vec_clone`),
  registered in `RUNTIME_FUNCTIONS` and the LLVM `runtime_decl.rs`
  (Cranelift infers the sig from the call site). New typeck arms
  in `typeck/infer.rs::builtin_method_type` cover all five —
  `fold` returns the resolved init type, `all` / `any` return
  `Bool`, `take` / `skip` return the same `*Iter` for chaining.
  The pre-existing rejection in `codegen/runtime.rs::runtime_name`
  for `take` / `skip` was lifted; `fold` / `all` / `any` stay in
  the rejection list because they never reach codegen (the inliner
  short-circuits before mangle). Updated
  `crates/ruxen-core/tests/codegen_unknown_method_rejected.rs` —
  the canary test that pinned `iter.fold` rejection now pins
  `iter.zip` rejection (still unimplemented — `chain` / `zip` /
  `collect[FromIterator]` are the deferred surface for #05 batch 3).
  New unit-test file `crates/ruxen-core/tests/stdlib_iterator.rs`
  drives the full lex → parse → typeck → MIR → Cranelift codegen
  pipeline in-process (no `cc` link, no temp files, no `ruxenc`
  subprocess) and runs in ~30 ms total across 14 tests; this is
  the primary TDD loop for #05 (the e2e probe remains the
  end-to-end confirmation). Three new release-e2e fixtures
  `603_iter_fold.rx` / `604_iter_all_any.rx` /
  `605_iter_take_skip.rx` confirm runtime behaviour
  (`PASS=208 / 208` on `release_e2e_smoke`). Still deferred to a
  later batch: `chain` / `zip` (need real iterator structs holding
  two sources), `collect[C: FromIterator]` (needs the
  `FromIterator` mixin + include machinery; a v1 `iter.collect_vec`
  shorthand is the planned escape hatch), and lifting the surface
  into a real `.rx` `mixin Iterator` source (needs a stdlib
  loader, not yet built).
- Phase 2 stdlib `Map[K, V]` indexing operator and Hash-key
  constraint negatives (#04 batch 3). `m[k]` now lowers through
  `ruxen_hash_index` and panics with `"hashmap index: missing key"`
  on miss (mirrors `Array[i]` / `ruxen_vec_get_or_panic`). The MIR
  `Index` handler in `mir/lower.rs` was extended to recognise
  `Ty::HashMap(_, _)` and `Ty::Ref(HashMap)` receivers; `infer_index_ty`
  in `typeck/infer.rs` was changed from `Ty::Option(V)` to `V` to
  match the panicking-index surface. Resolver-time validation in
  `resolve/mod.rs::ty_is_valid_hash_key` rejects compound containers
  (`Array`, `Set`, `Map`) as `Map` keys / `Set` elements, emitting
  `E0615` at the type-construction site (parallel to the per-field
  auto-synth validator in `implicit_includes/mod.rs`). New release-e2e fixture
  `tests/release-e2e/cases/509_map_index_op.rx` exercises
  the hit path; six new negative tests in
  `crates/ruxen-core/tests/stdlib_map_negatives.rs`
  (`hashmap_with_non_hash_key_emits_e0615`,
  `hashset_with_non_hash_element_emits_e0615`,
  `hashmap_with_nested_compound_key_emits_e0615`,
  `hashset_of_hashmap_emits_e0615`, plus two accept-path sanity
  checks) pin the typeck-level diagnostic.
- Phase 2 stdlib `Map[K, V]` + `Set[T]` per-element drop
  selectors (#04 batch 2). Five new runtime helpers
  (`ruxen_hash_drop_string_v`, `ruxen_hash_drop_v_string`,
  `ruxen_hash_drop_string_string`, `ruxen_hash_drop_v_vec`,
  `ruxen_set_drop_string`) walk the bucket chains and release the
  heap-owned key/value/element before delegating to the spine free.
  New runtime helper `ruxen_set_free` (paired with `ruxen_set_new`)
  closes the `Set` spine-leak gap that batch 1 deferred. The MIR
  drop-elaboration in `mir/lower.rs::insert_drops` now dispatches on
  `Ty::HashMap(K, V)` and `Ty::Set(T)` to pick the right helper based
  on whether K/V/T own heap. Push-time ownership transfer extended
  to taint BOTH the key (idx 1) and value (idx 2) of
  `ruxen_hash_insert` (and the value of `ruxen_set_insert`) so source
  `String.from(...)` / `Array.new` temps don't double-free with the
  per-element drop walk. Four new leak regression tests in
  `crates/ruxen-core/tests/drop_fixtures.rs`
  (`p04_hashmap_string_to_int_releases_every_key`,
  `p04_hashmap_int_to_string_releases_every_value`,
  `p04_hashmap_string_to_vec_int_releases_every_value`,
  `p04_hashset_string_releases_every_element`).
- Phase 2 stdlib `Map[K, V]` + `Set[T]` full surface (#04). New
  `Map` methods (runtime + Cranelift sig + LLVM extern + dispatch):
  `with_capacity(Int)`, `remove(&K) -> Option[V]`, `clear`, `keys ->
  Array[&K]`, `values -> Array[&V]`, `iter -> Array[&K]`, plus `==` /
  `!=` routed through `ruxen_hash_eq` (mirrors `ruxen_vec_eq` from
  #03). New `Set` methods: `with_capacity`, `remove(&T) -> Bool`,
  `clear`, `iter -> Array[&T]`, set operations
  `union(&Self) -> Set[T]`, `intersection(&Self) -> Set[T]`,
  `difference(&Self) -> Set[T]`, plus `==` via `ruxen_set_eq`.
  Set-op helpers and `Map`
  container-returning helpers (`keys`, `values`, `iter`, `remove`,
  `with_capacity`) are added to the `FRESH_ALLOC_CALLEES` whitelist
  in `mir/lower.rs` so their fresh allocations are dropped at scope
  exit. 13 new release-e2e fixtures at
  `tests/release-e2e/cases/50[1-6]_map_*.rx` and
  `52[1-7]_set_*.rx`; new typecheck-level pin tests in
  `crates/ruxen-core/tests/stdlib_map.rs` and `stdlib_set.rs`.
- Phase 2 stdlib `Array[T]` surface batch 2 (#03): closes the closure
  surface and wires the per-element drop selector. New methods:
  `Array.from_iter(I)` static constructor (runtime fn
  `ruxen_vec_from_iter`, registered as an `Array` static method
  alongside `new` / `with_capacity`); `dedup` (runtime fn
  `ruxen_vec_dedup`, removes consecutive bitwise-equal slots);
  `sort_by(closure)` and `retain(closure)` (inlined at MIR via the
  existing closure-method machinery; `sort_by` lowers to a
  selection-sort druxen by the user comparator, `retain` lowers to
  a read-write cursor loop using the new runtime helper
  `ruxen_vec_set`). MIR drop-elaboration now selects per-element
  drop helpers based on element type: `Array[String]` →
  `ruxen_vec_drop_string`, `Array[Array[T]]` → `ruxen_vec_drop_vec`,
  primitives still use the spine-only `ruxen_vec_free`. Push-time
  ownership transfer (`ruxen_vec_push` / `ruxen_vec_insert` /
  `ruxen_hash_insert`) now taints the value-arg local so the drop
  pass does not double-free the heap that the receiving slot
  inherits. Three new release-e2e fixture pairs at
  `tests/release-e2e/cases/40[9-11]_*` plus two new drop-leak
  regression tests in `crates/ruxen-core/tests/drop_fixtures.rs`
  (`vec_of_string_releases_every_element`,
  `vec_of_vec_int_releases_every_inner_vec`). New typeck negatives
  pinning `dedup`, `retain`, `sort_by`, and `from_iter` contracts.
  New developer-facing reference at
  `docs/dev/array_iter_borrow_rules.md` documenting the receiver-mode
  rules that statically reject iterator–mutator interleavings.
- Phase 2 stdlib `Array[T]` surface batch 1 (#03): new constructors and
  inspectors `Array.with_capacity(Int)`, `capacity`; new mutators
  `clear`, `truncate(Int)`, `swap(Int, Int)`, `insert(Int, T)`,
  `remove(Int) -> T`, `extend(&Array[T])`; new conversions / iter
  surface stubs `as_slice`, `iter_mut` (both passthrough on the v1
  RuxenVec representation); operator wiring `Array[T] == Array[T]` /
  `!=` (routed through `ruxen_vec_eq`, replacing the prior
  pointer-compare); indexing `v[i]` (routed through
  `ruxen_vec_get_or_panic`, panic message `"index N out of range,
  len M"`). Per-element drop helpers `ruxen_vec_drop_string` and
  `ruxen_vec_drop_vec` ship as runtime fns ready for the drop
  selector wiring (closes the runtime half of the `Array[String]`
  spine-only limitation; full MIR selector lands in batch 2). Eight
  new release-e2e fixture pairs at `tests/release-e2e/cases/40[1-8]_*`.
  New negatives suite `crates/ruxen-core/tests/stdlib_array_negatives.rs`
  pinning the typecheck contract for `Array[i]`, `Array.pop -> Option`,
  `Array[T] == Array[T] -> Bool`, plus two TODO-tagged tests recording
  current typeck laxness for `Array.from(_)` / non-Int args to
  `with_capacity`.
- Phase 2 stdlib `String` surface batch 2 (#02): closes the surface
  gap left by batch 1. New runtime fns `ruxen_string_split`,
  `ruxen_string_push`, `ruxen_string_into_bytes` (registered in the
  Cranelift signatures, LLVM externs, and `RUNTIME_FUNCTIONS`
  dispatch). New language wiring: `String.split(&str) -> Array[String]`
  (standalone, distinct from `splitn`); `String.push(Char)` (now
  routed through the dedicated runtime fn so the codepoint
  intermediate doesn't leak); `String.into_bytes -> Array[UInt8]` (the
  consuming variant of `bytes` — frees the source spine internally
  and the dealloc-safety analysis taints the receiver to avoid a
  double-free); `String + String` (concat owned, reuses
  `ruxen_string_concat`); `String += String` (push_str-style
  in-place, reuses `ruxen_string_push_str`). Five new release-e2e
  fixture pairs at `tests/release-e2e/cases/31[4-8]_*`. Three new
  drop-leak regression tests in `crates/ruxen-core/tests/drop_fixtures.rs`
  (`string_push_does_not_leak`, `string_into_bytes_transfers_ownership`,
  `string_plus_op_frees_both_operands`). New negatives suite
  `crates/ruxen-core/tests/stdlib_string_negatives.rs` pinning the
  typecheck gaps around static-method arg validation and
  borrow-after-move on owned String args.
- Phase 2 stdlib `String` surface batch 1 (#02): new constructors
  `String.new` and `String.with_capacity(Int)`; new inspectors
  `as_str`, `bytes`, `find(&str) -> Option[Int]`, `splitn(Int, &str)`,
  `trim_start`, `trim_end`; new mutators `clear`, `truncate(Int)`,
  `insert(Int, Char)`, `insert_str(Int, &str)`, `remove(Int) -> Char`;
  new conversions `to_string`, `parse[Int] -> Result[Int, ParseIntError]`,
  `parse[Float] -> Result[Float, ParseFloatError]`. Each method has a
  matching `ruxen_string_<op>` runtime fn (declared in
  `runtime.c`, registered in Cranelift+LLVM signatures and
  `RUNTIME_FUNCTIONS`), MIR dispatch in `mir/lower.rs`, and a
  release-e2e fixture pair under `tests/release-e2e/cases/3NN_*`.
  Surface gap remaining for batch 2: `split` (standalone), `push(Char)`,
  `into_bytes`, `+` and `+=` operators.
- `ruxen explain ECODE` subcommand — looks up a compiler error code
  in the central registry and prints its title (T5.04 phase 2).
- Long-form `ruxen explain ECODE` output: every registered code now
  has a markdown file under `docs/errors/<code>.md` (Why / Example /
  Fix sections) embedded into the binary via `include_str!`. The
  CLI prints the full explanation when available and falls back to
  title-only with a note when not. A registry-coverage test enforces
  that the markdown table stays in sync with
  `ruxen_core::diagnostics::codes::REGISTRY` (T5.04 phase 3, #01-C).
- `crates/ruxen-core/src/diagnostics/codes.rs` — central registry
  for every emitted compiler error code (T5.04 phase 1).
- CI workflow with build/test/MSRV gating + advisory lint job
  (T4.06 phase 1).
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md` (T4.08).
- Drop elaboration: heap-owned locals freed on reassignment and at
  every loop-exit edge (break, continue, back-edge) (P0.2).
- Registered auto-synth error codes E0601, E0603, E0605, E0608, E0609
  (previously emitted by `implicit_includes/mod.rs` but missing from the public
  registry) and reserved E0610, E0611, E0613, E0615, E0616, E0617,
  E0618 ahead of T1.05 auto-synth work (#01-B1).
- Auto-synthesized `Clone` now produces a working `<Type>_clone` for
  structs, classes, and enums (including variants with payloads).
  Per-field clone dispatches to bitwise copy for primitives,
  `ruxen_string_clone` for `String`, `ruxen_vec_clone` /
  `ruxen_hash_clone` / `ruxen_set_clone` for built-in containers, and
  recursively to `<Inner>_clone` for user types that themselves
  satisfy `Clone`. Non-`Clone` field/payload types are rejected with
  `E0610` at validation time (T1.05 #01-B2).
- Remaining built-in auto-synth mixins — `Debug`, `PartialEq`, `Eq`,
  `Hash`, `Default`, `Ord`, `PartialOrd`, `Copy` — now produce
  working code rather than validate-only skeletons (T1.05 #01-B3):
  struct ordering operators (`<`, `<=`, `>`, `>=`) dispatch through
  the synthesised `<Type>_cmp` / `<Type>_partial_cmp` so multi-field
  structs are ordered lex-tuple style instead of pointer-compared.
  Per-field bound checks emit `E0613` (PartialEq), `E0615` (Hash),
  `E0617` (Ord), and `E0618` (PartialOrd) when an inner field type
  doesn't satisfy the mixin, and `E0616` fires when `Default` is
  auto-synthesized on an empty enum. Six new release-e2e fixtures
  (`tests/release-e2e/cases/201`, `206`, `207`, `208`, `209`)
  exercise the green path; five negatives in
  `crates/ruxen-core/tests/implicit_negatives.rs` pin the red path.

### Fixed
- Heap-owned `String`, `Array`, and `Map` locals are now freed on
  scope exit (P0.7). Previously these types leaked until program
  exit; drop elaboration only released `Class`/`Struct`/`Enum`
  storage. Three new runtime helpers (`ruxen_string_free`,
  `ruxen_vec_free`, `ruxen_hash_free`) release the spine of each
  owning type. (#01-A)
- Process-level argv copy from `ruxen_env_init` is released via
  `atexit` so leak-tracking test harnesses see a clean exit ledger.

### Known limitations
- `Array[String]` and `Map[K, V]` with heap-owned element types
  only free the spine (data buffer + outer struct). Element heap is
  leaked; recursive element drops are deferred to a future prompt.
  *Update (#04 batch 2):* `Map[String, V]`, `Map[K, String]`,
  `Map[String, String]`, `Map[K, Array[T]]`, and
  `Set[String]` now release every owned key/value/element via
  the per-element drop selectors `ruxen_hash_drop_string_v` /
  `ruxen_hash_drop_v_string` / `ruxen_hash_drop_string_string` /
  `ruxen_hash_drop_v_vec` / `ruxen_set_drop_string`. Deeper nesting
  (Map-in-Map, Set-in-V, etc.) is still spine-only and
  lands with the mixin-druxen drop dispatch in #05.

### Changed
- `LoopFrame` now tracks `body_locals` for the drop pass.
- Un-reserved keyword `spawn` (`actor`, `send`, `receive` were
  un-reserved in pre-Phase-1 work). Decision: ship the async-only
  path; actors are deferred to v2 as a library, then language v2.0.
  `async` / `await` remain reserved for Phase 4. (P0.12)

### Notes
- Initial public preview was first cut 2026-04-23 and is folded into this
  0.1.0 release. See `docs/requirements/ROADMAP.md` for the roadmap to 1.0.
